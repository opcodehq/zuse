//! Unknown GitHub identities must explicitly choose in the OAuth browser, not
//! merely in the app holding the initiating WorkOS bearer (login CSRF boundary).
use super::*;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSelection {
    pub state: String,
    pub installation_id: i64,
}
pub struct BrowserTransaction {
    pub actor: String,
    pub owner: String,
    pub operation_id: String,
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
impl NativeGithub {
    pub fn browser_origin_matches(&self, origin: &str) -> bool {
        origin == self.config.origin.origin().ascii_serialization()
    }
    pub(in crate::github::native) async fn browser_row(
        &self,
        state: &str,
        browser: Option<&str>,
    ) -> Result<sqlx::postgres::PgRow, ApiError> {
        let browser = browser
            .filter(|s| s.len() == 64)
            .ok_or_else(|| ApiError::forbidden("github_browser_mismatch"))?;
        if state.len() != 64 {
            return Err(ApiError::bad_request("invalid_github_state"));
        }
        let row=sqlx::query("SELECT o.operation_id,o.actor_id,o.owner_id,o.phase,o.sealed_pending FROM zuse_rust_github_operations o JOIN zuse_rust_github_users u ON u.actor_id=o.actor_id JOIN zuse_rust_github_scopes s ON s.owner_id=o.owner_id WHERE o.state_hash=$1 AND o.browser_hash=$2 AND o.expires_at>$3 AND ((o.actor_epoch=u.epoch AND o.scope_epoch=s.epoch AND u.state<>'revoking') OR o.phase='complete')")
            .bind(hash(state)).bind(hash(browser)).bind(now()).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::forbidden("github_browser_mismatch"))?;
        let owner: String = row.try_get("owner_id")?;
        if !self.enabled(&owner) {
            return Err(ApiError::forbidden("github_native_disabled"));
        }
        Ok(row)
    }
    pub async fn browser_page(
        &self,
        state: &str,
        browser: Option<&str>,
    ) -> Result<String, ApiError> {
        let row = self.browser_row(state, browser).await?;
        let id: String = row.try_get("operation_id")?;
        let phase: String = row.try_get("phase")?;
        if phase == "complete" {
            return Ok(page(
                "GitHub connected",
                "Return to Zuse to choose repositories and continue.",
                "",
            ));
        }
        if phase != "choosing" {
            return Err(ApiError::conflict("github_operation_unavailable"));
        }
        let pending: Pending = self.open(
            &format!("github-pending:{id}"),
            &row.try_get::<String, _>("sealed_pending")?,
        )?;
        let user = public_user(&pending.profile)?;
        let mut choices = String::new();
        for page_number in 1..=100 {
            let response = self
                .get(
                    &format!("/user/installations?per_page=100&page={page_number}"),
                    &pending.credentials.access_token,
                )
                .await?;
            let entries = response["installations"]
                .as_array()
                .ok_or_else(unavailable)?;
            for installation in entries {
                let Some(installation_id) = installation["id"].as_i64().filter(|n| *n > 0) else {
                    continue;
                };
                if pending
                    .browser_installation
                    .is_some_and(|id| id != installation_id)
                {
                    continue;
                }
                if !installation["suspended_at"].is_null() {
                    continue;
                }
                let Some(login) = installation["account"]["login"]
                    .as_str()
                    .filter(|s| valid_component(s))
                else {
                    continue;
                };
                choices.push_str(&format!("<form method=post action='/v1/cloud/github/browser/select'><input type=hidden name=state value='{}'><input type=hidden name=installationId value='{installation_id}'><button type=submit>Use {}</button></form>",escape(state),escape(login)));
            }
            if entries.len() < 100 {
                break;
            }
            if page_number == 100 {
                return Err(ApiError::unavailable("github_pagination_limit"));
            }
        }
        let owner: String = row.try_get("owner_id")?;
        let workspace = if owner.starts_with("organization:") {
            format!(
                "organization workspace {}",
                escape(owner.trim_start_matches("organization:"))
            )
        } else {
            "Personal workspace".into()
        };
        let description = format!(
            "Connect GitHub account <strong>{}</strong> to your {workspace} in Zuse. Continue only if you started this connection from your own Zuse app.",
            escape(&user.login)
        );
        if choices.is_empty() {
            choices.push_str("<p>No installations are available. An organization administrator may need to install the App or approve its access.</p>");
        }
        if pending.browser_installation.is_none() || pending.browser_installation == Some(0) {
            choices.push_str(&format!("<form method=post action='/v1/cloud/github/browser/identity'><input type=hidden name=state value='{}'><button type=submit>Connect account while waiting for App approval</button></form>",escape(state)));
        }
        choices.push_str(&format!("<p><a href='{}' target=_blank rel='noreferrer noopener'>Install GitHub App or request approval</a></p><p><a href='/v1/cloud/github/browser?state={}'>Check approval and refresh</a></p>",escape(&self.install_url(state)),escape(state)));
        Ok(page("Choose a GitHub installation", &description, &choices))
    }
    pub fn browser_failure_page(&self, state: &str, code: &str) -> String {
        let (title, hint) = match code {
            "github_organization_approval_required" => (
                "Organization approval required",
                "Ask your GitHub organization owner to approve Members read access for the App. They do not need a Zuse account. Return here and check approval afterward.",
            ),
            "github_installation_admin_required" => (
                "Connect the workspace installation first",
                "Ask a Zuse organization administrator to link this GitHub installation. You can then connect your own GitHub identity to its repositories.",
            ),
            "github_organization_membership_required" | "github_repository_access_denied" => (
                "Repository access is required",
                "Your GitHub account needs active organization membership and writable access to the selected repositories.",
            ),
            _ => (
                "GitHub connection is not finished",
                "Your connection has been preserved for this session. Check the installation and try again, or cancel and start a new connection in Zuse.",
            ),
        };
        page(
            title,
            hint,
            &format!(
                "<p><a href='/v1/cloud/github/browser?state={}'>Check approval and retry</a></p>",
                escape(state)
            ),
        )
    }
    /// The browser's cookie is mandatory even when the app knows its own state
    /// and bearer. Once chosen, another installation cannot replace this consent.
    pub async fn confirm_browser(
        &self,
        selection: &BrowserSelection,
        browser: Option<&str>,
    ) -> Result<BrowserTransaction, ApiError> {
        if selection.installation_id <= 0 {
            return Err(ApiError::bad_request("invalid_github_installation"));
        }
        let row = self.browser_row(&selection.state, browser).await?;
        let id: String = row.try_get("operation_id")?;
        let phase: String = row.try_get("phase")?;
        if phase != "choosing" {
            return Err(ApiError::conflict("github_operation_unavailable"));
        }
        let sealed: String = row.try_get("sealed_pending")?;
        let mut pending: Pending = self.open(&format!("github-pending:{id}"), &sealed)?;
        if pending
            .browser_installation
            .is_some_and(|id| id != selection.installation_id)
        {
            return Err(ApiError::conflict("github_browser_choice_changed"));
        }
        pending.browser_installation = Some(selection.installation_id);
        let next = self.seal(&format!("github-pending:{id}"), &pending)?;
        let updated=sqlx::query("UPDATE zuse_rust_github_operations SET sealed_pending=$3 WHERE operation_id=$1 AND phase='choosing' AND sealed_pending=$2 AND expires_at>$4")
            .bind(&id).bind(sealed).bind(next).bind(now()).execute(&self.pool).await?;
        if updated.rows_affected() != 1 {
            return Err(ApiError::conflict("github_operation_unavailable"));
        }
        Ok(BrowserTransaction {
            actor: row.try_get("actor_id")?,
            owner: row.try_get("owner_id")?,
            operation_id: id,
        })
    }
}
fn page(title: &str, description: &str, choices: &str) -> String {
    format!(
        "<!doctype html><html lang=en><head><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>{title} · Zuse</title><style>body{{margin:0;background:#121318;color:#f2f3f7;font:15px system-ui,sans-serif;display:grid;min-height:100vh;place-items:center}}main{{width:min(440px,calc(100% - 64px));padding:32px;background:#1c1e25;border:1px solid #343742;border-radius:16px}}h1{{font-size:22px;margin:0 0 12px}}p{{line-height:1.6;color:#b7bbc8}}form{{margin:12px 0}}button{{width:100%;text-align:left;padding:14px 16px;border:1px solid #4d5263;border-radius:9px;background:#2b2f3a;color:#fff;font:inherit;cursor:pointer}}button:hover,button:focus{{background:#383e4c;border-color:#9ca8ff}}a{{color:#b6bfff}}</style></head><body><main><h1>{title}</h1><p>{description}</p>{choices}</main></body></html>"
    )
}
