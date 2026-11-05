//! Host-owned credentials. The login mechanism stays outside the harness.
//! Secrets never enter RunRequest, session history or diagnostic output.
use async_trait::async_trait;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::{HarnessError, jsonrpc::RpcClient};

pub enum Credential {
    ApiKey(Zeroizing<String>),
    ChatGpt {
        access_token: Zeroizing<String>,
        account_id: String,
        plan_type: Option<String>,
    },
}

#[async_trait]
pub trait CredentialResolver: Send + Sync {
    /// Ephemeral task environment, independent of account/login/start credentials.
    async fn environment(&self) -> Result<crate::credentials::ChildEnvironment, HarnessError> {
        Ok(crate::credentials::ChildEnvironment::empty())
    }
    /// `previous_account` fences renewal to the original selected account.
    /// Unauthorized requests must bypass an otherwise unexpired cached token.
    async fn resolve(
        &self,
        unauthorized: bool,
        previous_account: Option<&str>,
    ) -> Result<Credential, HarnessError>;
    /// Recheck the selected authority connection rather than trusting a cached
    /// grant when a retained app-server admits a new turn.
    async fn before_turn(&self) -> Result<Credential, HarnessError> {
        self.resolve(false, None).await
    }
}

impl Credential {
    fn login_params(&self) -> Value {
        match self {
            Self::ApiKey(key) => json!({"type":"apiKey", "apiKey":key.as_str()}),
            Self::ChatGpt {
                access_token,
                account_id,
                plan_type,
            } => json!({
                "type":"chatgptAuthTokens", "accessToken":access_token.as_str(),
                "chatgptAccountId":account_id, "chatgptPlanType":plan_type
            }),
        }
    }
    fn refresh_params(&self) -> Result<Value, HarnessError> {
        let Self::ChatGpt {
            access_token,
            account_id,
            plan_type,
        } = self
        else {
            return Err(failure());
        };
        Ok(json!({"accessToken":access_token.as_str(),
            "chatgptAccountId":account_id, "chatgptPlanType":plan_type}))
    }
}

fn failure() -> HarnessError {
    HarnessError::Protocol("codex-auth-reconnecting".into())
}

pub(super) async fn login(
    client: &RpcClient,
    resolver: &dyn CredentialResolver,
) -> Result<(), HarnessError> {
    tokio::time::timeout(std::time::Duration::from_secs(45), async {
        let credential = resolver.resolve(false, None).await.map_err(|_| failure())?;
        // App-server errors may contain request contents. Never forward those errors.
        client
            .request("account/login/start", credential.login_params())
            .await
            .map_err(|_| failure())?;
        Ok(())
    })
    .await
    .map_err(|_| failure())?
}

pub(super) async fn login_before_turn(
    client: &RpcClient,
    resolver: &dyn CredentialResolver,
) -> Result<(), HarnessError> {
    tokio::time::timeout(std::time::Duration::from_secs(45), async {
        let credential = resolver.before_turn().await.map_err(|_| failure())?;
        client
            .request("account/login/start", credential.login_params())
            .await
            .map_err(|_| failure())?;
        Ok(())
    })
    .await
    .map_err(|_| failure())?
}

pub(super) async fn refresh(
    resolver: &dyn CredentialResolver,
    params: &Value,
) -> Result<Value, HarnessError> {
    let previous = match params.get("previousAccountId") {
        None | Some(Value::Null) => None,
        Some(Value::String(id)) if !id.is_empty() && id.len() <= 256 => Some(id.as_str()),
        _ => return Err(failure()),
    };
    tokio::time::timeout(std::time::Duration::from_secs(45), async {
        let credential = resolver
            .resolve(true, previous)
            .await
            .map_err(|_| failure())?;
        if let (Some(previous), Credential::ChatGpt { account_id, .. }) = (previous, &credential)
            && previous != account_id
        {
            return Err(failure());
        }
        credential.refresh_params()
    })
    .await
    .map_err(|_| failure())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Resolver {
        account: &'static str,
        calls: Mutex<Vec<(bool, Option<String>)>>,
    }
    #[async_trait]
    impl CredentialResolver for Resolver {
        async fn resolve(
            &self,
            unauthorized: bool,
            previous: Option<&str>,
        ) -> Result<Credential, HarnessError> {
            self.calls
                .lock()
                .unwrap()
                .push((unauthorized, previous.map(str::to_owned)));
            Ok(Credential::ChatGpt {
                access_token: Zeroizing::new("synthetic-access".into()),
                account_id: self.account.into(),
                plan_type: Some("test-plan".into()),
            })
        }
    }
    #[tokio::test]
    async fn unauthorized_renewal_is_account_fenced_and_rejects_malformed_requests() {
        let resolver = Resolver {
            account: "account-a",
            calls: Mutex::new(vec![]),
        };
        let result = refresh(&resolver, &json!({"previousAccountId":"account-a"}))
            .await
            .unwrap();
        assert_eq!(result["accessToken"], "synthetic-access");
        assert!(result.get("refreshToken").is_none());
        assert!(
            refresh(&resolver, &json!({"previousAccountId":"account-b"}))
                .await
                .is_err()
        );
        assert!(
            refresh(&resolver, &json!({"previousAccountId":42}))
                .await
                .is_err()
        );
        assert_eq!(
            *resolver.calls.lock().unwrap(),
            vec![
                (true, Some("account-a".into())),
                (true, Some("account-b".into()))
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn brokered_api_key_auth_uses_memory_storage_and_clears_inherited_credentials() {
        let harness =
            super::super::CodexHarness::new().with_credentials(std::sync::Arc::new(Resolver {
                account: "account-a",
                calls: Mutex::new(vec![]),
            }));
        let mut command = crate::process::Command::new("codex");
        harness.isolate_credentials(&mut command);
        assert_eq!(
            command.as_std().get_args().collect::<Vec<_>>(),
            vec!["--config", "cli_auth_credentials_store=\"ephemeral\""]
        );
        for key in ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN"] {
            assert!(
                command
                    .as_std()
                    .get_envs()
                    .any(|(name, value)| name == key && value.is_none())
            );
        }
        assert_eq!(
            Credential::ApiKey(Zeroizing::new("synthetic-key".into())).login_params(),
            json!({"type":"apiKey", "apiKey":"synthetic-key"})
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn brokered_model_probe_authenticates_before_discovery_and_redacts_login_errors() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("codex");
        std::fs::write(&script, r#"#!/usr/bin/env python3
import json,sys
logged_in=False
for line in sys.stdin:
    request=json.loads(line)
    method=request.get('method')
    if method=='initialize': result={}
    elif method=='initialized': continue
    elif method=='account/login/start':
        assert request['params']=={'type':'chatgptAuthTokens','accessToken':'synthetic-access','chatgptAccountId':'account-a','chatgptPlanType':'test-plan'}
        logged_in=True
        result={}
    elif method=='model/list':
        assert logged_in
        result={'data':[{'id':'test-model','model':'test-model','displayName':'Test','description':'Test','isDefault':True,'supportedReasoningEfforts':[],'defaultReasoningEffort':'medium'}]}
    else: raise Exception('Unexpected method')
    print(json.dumps({'id':request['id'],'result':result}),flush=True)
"#).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let harness = super::super::CodexHarness::new()
            .with_executable(&script)
            .with_credentials(std::sync::Arc::new(Resolver {
                account: "account-a",
                calls: Mutex::new(vec![]),
            }));
        let models = harness.discover_models().await.unwrap();
        assert_eq!(models[0].id, "test-model");
        let text = std::fs::read_to_string(&script).unwrap().replace("logged_in=True\n        result={}",
            "print(json.dumps({'id':request['id'],'error':{'code':-1,'message':'synthetic-access'}}),flush=True)\n        continue");
        std::fs::write(&script, text).unwrap();
        let error = harness.discover_models().await.unwrap_err().to_string();
        assert!(error.contains("codex-auth-reconnecting"));
        assert!(!error.contains("synthetic-access"));
    }
}
