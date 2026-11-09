//! Installation callbacks are routing hints, not authorization. Native linking
//! still requires the same cookie-bound browser consent and current user access.
use super::*;
#[derive(Deserialize)]
pub struct SetupCallback {
    pub state: Option<String>,
    pub installation_id: Option<i64>,
    pub setup_action: Option<String>,
}
pub fn legacy_setup_forward(
    legacy: &crate::github::LegacyGithub,
    query: &SetupCallback,
) -> Result<Option<reqwest::Url>, ApiError> {
    let Some(state) = &query.state else {
        return Ok(None);
    };
    // Native random state has no dots and must never fall through to a different
    // authority when its cookie/epoch is absent. The fixed configured legacy API
    // remains responsible for verifying legacy signed state and issuer routing.
    if state.len() == 64 && !state.contains('.') {
        return Ok(None);
    }
    if state.len() > 16384 || state.split('.').count() != 3 {
        return Err(ApiError::bad_request("invalid_github_install_state"));
    }
    let payload = URL_SAFE_NO_PAD
        .decode(state.split('.').nth(1).unwrap_or_default())
        .map_err(|_| ApiError::bad_request("invalid_github_install_state"))?;
    let hint: Value = serde_json::from_slice(&payload)
        .map_err(|_| ApiError::bad_request("invalid_github_install_state"))?;
    let issuer = hint["iss"]
        .as_str()
        .filter(|issuer| {
            matches!(
                *issuer,
                "https://api.zuse.sh" | "https://api-staging.zuse.sh"
            )
        })
        .ok_or_else(|| ApiError::bad_request("invalid_github_install_issuer"))?;
    if !matches!(
        legacy.url.origin().ascii_serialization().as_str(),
        "https://api.zuse.sh" | "https://api-staging.zuse.sh"
    ) {
        return Err(ApiError::unavailable("github_legacy_setup_not_configured"));
    }
    let installation = query
        .installation_id
        .filter(|id| *id > 0)
        .ok_or_else(|| ApiError::bad_request("invalid_github_installation"))?;
    let mut url = reqwest::Url::parse(issuer).expect("exact allowlisted legacy issuer");
    url.set_path("/v1/cloud/github/callback");
    url.query_pairs_mut()
        .append_pair("state", state)
        .append_pair("installation_id", &installation.to_string());
    if let Some(action) = &query.setup_action {
        if !matches!(action.as_str(), "install" | "update" | "request") {
            return Err(ApiError::bad_request("invalid_github_setup_action"));
        }
        url.query_pairs_mut().append_pair("setup_action", action);
    }
    Ok(Some(url))
}
impl NativeGithub {
    pub async fn setup_result(&self, query: &SetupCallback, browser: Option<&str>) -> String {
        if let Some(state) = &query.state {
            if state.len() == 64 {
                if let Ok(html) = self.browser_page(state, browser).await {
                    return html;
                }
            }
        }
        // Expired/missing state never implicitly links the installation or reveals
        // whether another account has a pending transaction.
        setup_notice()
    }
    pub(in crate::github::native) fn install_url(&self, state: &str) -> String {
        let mut url = reqwest::Url::parse(&format!(
            "https://github.com/apps/{}/installations/new",
            self.config.slug
        ))
        .expect("validated GitHub slug");
        url.query_pairs_mut().append_pair("state", state);
        url.to_string()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installation_hint_never_authorizes_or_selects_a_destination() {
        let bridge = crate::github::LegacyGithub::new("https://api.zuse.sh").unwrap();
        let mut query = SetupCallback {
            state: Some("opaque".repeat(10) + "rest"),
            installation_id: Some(9),
            setup_action: Some("install".into()),
        };
        assert!(legacy_setup_forward(&bridge, &query).unwrap().is_none());
        query.state = Some(format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&json!({"iss":"https://api.zuse.sh"})).unwrap())
        ));
        let url = legacy_setup_forward(&bridge, &query).unwrap().unwrap();
        assert_eq!(url.host_str(), Some("api.zuse.sh"));
        assert_eq!(url.path(), "/v1/cloud/github/callback");
        query.state = Some(format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&json!({"iss":"https://api-staging.zuse.sh"})).unwrap())
        ));
        assert_eq!(
            legacy_setup_forward(&bridge, &query)
                .unwrap()
                .unwrap()
                .host_str(),
            Some("api-staging.zuse.sh")
        );
        query.state = Some(format!(
            "header.{}.signature",
            URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&json!({"iss":"https://attacker.example"})).unwrap())
        ));
        assert!(legacy_setup_forward(&bridge, &query).is_err());
        query.setup_action = Some("https://attacker.example".into());
        assert!(legacy_setup_forward(&bridge, &query).is_err());
    }
}

pub fn setup_notice() -> String {
    "<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content='width=device-width,initial-scale=1'><title>GitHub installation updated · Zuse</title><body style='background:#121318;color:#f2f3f7;font:16px system-ui;padding:48px'><main style='max-width:440px;margin:10vh auto'><h1>GitHub installation updated</h1><p>Return to Zuse and refresh GitHub. If organization approval is still pending, check again after an administrator approves it.</p><p>You can close this tab.</p></main></body></html>".into()
}
