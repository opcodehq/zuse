//! Linking a fresh GitHub identity needs both halves of the transaction.
//!
//! The browser that completed GitHub OAuth proves who the GitHub user is, but
//! anyone can open a connect link that someone else started. The app holding
//! the initiating bearer proves which Zuse account asked, but cannot prove that
//! its user is the one in the browser. So after the browser chooses, it shows a
//! one-time code and nothing is linked until the initiating app submits it. A
//! visitor who opened another person's link would have to read that code out
//! to them, past a page that names the Zuse account it would connect to.
use super::*;
use subtle::ConstantTimeEq;
use zuse_browser_page::{Card, Tone};

/// Code alphabet without look-alike characters (no 0/O, 1/I/L, U).
const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTVWXYZ23456789";
const CODE_LENGTH: usize = 8;
/// Wrong codes allowed before the operation is failed and must restart.
pub(super) const MAX_ATTEMPTS: u8 = 5;

#[derive(Serialize, Deserialize)]
pub(super) struct Confirmation {
    /// Plain inside the sealed pending record, next to the OAuth token it guards.
    pub(super) code: String,
    #[serde(default)]
    pub(super) attempts: u8,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfirmation {
    pub operation_id: String,
    pub confirmation_code: String,
}

pub(super) fn new_code() -> String {
    let mut code = String::with_capacity(CODE_LENGTH);
    while code.len() < CODE_LENGTH {
        for byte in uuid::Uuid::new_v4().into_bytes() {
            // Rejection sampling keeps every character equally likely.
            let limit = 256 - (256 % ALPHABET.len());
            if (byte as usize) < limit && code.len() < CODE_LENGTH {
                code.push(ALPHABET[byte as usize % ALPHABET.len()] as char);
            }
        }
    }
    code
}

/// Accepts the code as displayed (`ABCD-EFGH`), in any case, with spacing.
pub(super) fn normalize(input: &str) -> Option<String> {
    if input.len() > 64 {
        return None;
    }
    let code: String = input
        .chars()
        .filter(|c| !matches!(c, '-' | ' ' | '\t'))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (code.len() == CODE_LENGTH && code.bytes().all(|b| ALPHABET.contains(&b))).then_some(code)
}

pub(super) fn matches(expected: &str, input: &str) -> bool {
    normalize(input).is_some_and(|code| bool::from(code.as_bytes().ct_eq(expected.as_bytes())))
}

fn display(code: &str) -> String {
    let (head, tail) = code.split_at(code.len() / 2);
    format!("{head}-{tail}")
}

pub(super) fn page(
    code: &str,
    github_login: &str,
    account: &str,
    workspace: &str,
    installation: Option<i64>,
) -> String {
    let mut facts = vec![
        ("GitHub account", github_login.to_owned()),
        ("Zuse account", account.to_owned()),
        ("Zuse workspace", workspace.to_owned()),
    ];
    if let Some(installation) = installation {
        facts.push(("Installation", installation.to_string()));
    }
    facts.push(("Confirmation code", display(code)));
    zuse_browser_page::card(&Card {
        facts,
        hint: Some(
            "Only enter this code in your own Zuse app. Never share it: anyone with this code can connect your GitHub account to their Zuse account. If you did not start this from your own Zuse app, close this tab. Nothing has been connected.",
        ),
        ..Card::notice(
            "GitHub",
            "Confirm in Zuse",
            Tone::Waiting,
            "Enter this code in Zuse",
            "To finish connecting, type the confirmation code below into the Zuse app that started this connection. Check that the Zuse account shown is yours.",
        )
    })
}

impl NativeGithub {
    /// A reused identity is already the initiator's own; only a fresh OAuth
    /// identity needs the app to confirm. Returns whether confirmation is pending.
    pub(super) fn require_confirmation(&self, pending: &mut Pending) -> bool {
        if pending.reused {
            return false;
        }
        let attempts = pending.confirmation.as_ref().map_or(0, |c| c.attempts);
        pending.confirmation = Some(Confirmation {
            code: new_code(),
            attempts,
        });
        true
    }

    /// The phase stays `choosing`, so publish a feed hint for live app sessions.
    pub(super) async fn announce_confirmation(&self, transaction: &BrowserTransaction) {
        let events: Result<bool, _> =
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_account_event_outbox') IS NOT NULL")
                .fetch_one(&self.pool)
                .await;
        if !matches!(events, Ok(true)) {
            return;
        }
        let result = sqlx::query("INSERT INTO zuse_rust_account_event_outbox(owner_id,kind,resource_id,payload,created_at) VALUES($1,'github',$2,$3,$4)")
            .bind(&transaction.owner)
            .bind(&transaction.operation_id)
            .bind(json!({"phase":"choosing","actorId":transaction.actor,"confirmationRequired":true}))
            .bind(now())
            .execute(&self.pool)
            .await;
        if let Err(error) = result {
            tracing::warn!(%error, "GitHub confirmation event deferred");
        }
    }

    /// Completes a fresh identity link from the initiating app session.
    pub async fn confirm_app(
        &self,
        access: &Access,
        request: &AppConfirmation,
    ) -> Result<(), ApiError> {
        self.require(access)?;
        let id = &request.operation_id;
        let row=sqlx::query("SELECT sealed_pending FROM zuse_rust_github_operations WHERE operation_id=$1 AND actor_id=$2 AND owner_id=$3 AND phase='choosing' AND expires_at>$4")
            .bind(id).bind(&access.actor.account_id).bind(&access.owner_id).bind(now()).fetch_optional(&self.pool).await?.ok_or_else(||ApiError::conflict("github_operation_unavailable"))?;
        let sealed: String = row.try_get("sealed_pending")?;
        let mut pending: Pending = self.open(&format!("github-pending:{id}"), &sealed)?;
        let Some(confirmation) = pending.confirmation.as_mut() else {
            return Err(ApiError::conflict("github_confirmation_not_requested"));
        };
        let installation = pending
            .browser_installation
            .ok_or_else(|| ApiError::forbidden("github_browser_confirmation_required"))?;
        if confirmation.attempts >= MAX_ATTEMPTS {
            self.fail_confirmation(id).await?;
            return Err(ApiError::forbidden("github_confirmation_locked"));
        }
        // Reserve this attempt before comparing. The compare-and-swap on the
        // sealed record lets exactly one of several concurrent requests that
        // read the same record proceed; the rest are refused without a
        // comparison, so parallel guesses cannot share one attempt.
        let expected = confirmation.code.clone();
        confirmation.attempts += 1;
        let attempts = confirmation.attempts;
        let next = self.seal(&format!("github-pending:{id}"), &pending)?;
        let reserved=sqlx::query("UPDATE zuse_rust_github_operations SET sealed_pending=$3 WHERE operation_id=$1 AND phase='choosing' AND sealed_pending=$2")
            .bind(id).bind(&sealed).bind(next).execute(&self.pool).await?;
        if reserved.rows_affected() != 1 {
            return Err(ApiError::conflict("github_operation_unavailable"));
        }
        if !matches(&expected, &request.confirmation_code) {
            if attempts >= MAX_ATTEMPTS {
                self.fail_confirmation(id).await?;
                return Err(ApiError::forbidden("github_confirmation_locked"));
            }
            return Err(ApiError::forbidden("github_confirmation_invalid"));
        }
        if installation == 0 {
            self.finalize_identity_checked(access, id, true).await
        } else {
            self.select_checked(
                access,
                &Selection {
                    operation_id: id.clone(),
                    installation_id: installation,
                },
                true,
            )
            .await
        }
    }

    async fn fail_confirmation(&self, id: &str) -> Result<(), ApiError> {
        sqlx::query("UPDATE zuse_rust_github_operations SET phase='failed',sealed_pending=NULL WHERE operation_id=$1 AND phase='choosing'").bind(id).execute(&self.pool).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_uniform_unambiguous_and_tolerant_of_display_format() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..200 {
            let code = new_code();
            assert_eq!(code.len(), CODE_LENGTH);
            assert!(code.bytes().all(|b| ALPHABET.contains(&b)));
            seen.insert(code);
        }
        assert_eq!(seen.len(), 200, "codes must not repeat");
        let code = new_code();
        let shown = display(&code);
        assert!(matches(&code, &shown));
        assert!(matches(&code, &shown.to_ascii_lowercase()));
        assert!(matches(&code, &format!(" {} ", shown.replace('-', " "))));
        let mut wrong = code.clone().into_bytes();
        wrong[0] = if wrong[0] == b'A' { b'B' } else { b'A' };
        assert!(!matches(&code, std::str::from_utf8(&wrong).unwrap()));
        assert!(!matches(&code, ""));
        assert!(!matches(&code, &code[..CODE_LENGTH - 1]));
        assert!(!matches(&code, &"A".repeat(4096)));
    }

    #[test]
    fn confirmation_page_names_the_initiating_account_and_warns_against_sharing() {
        let html = page(
            "ABCDEFGH",
            "octo-visitor",
            "Owner Person <owner@example.test>",
            "Personal",
            Some(9),
        );
        assert!(html.contains("ABCD-EFGH"));
        assert!(html.contains("octo-visitor"));
        assert!(html.contains("owner@example.test"));
        assert!(html.contains("Never share it"));
        assert!(
            !html.contains("<form"),
            "the browser cannot complete the link itself"
        );
    }
}
