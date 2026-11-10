//! Cloud operations follow the account shown in General settings. Workspace auth
//! remains responsible for device/relay access and supplies the profile directory.
use crate::{
    account::Account,
    auth::{Auth, AuthState, AuthUser},
};
use std::path::Path;
use tokio::sync::watch;

#[derive(Clone)]
pub struct CloudIdentity {
    account: Option<Account>,
    workspace: Auth,
    hosted_scope: Option<watch::Receiver<(Option<String>, Option<String>)>>,
}
impl CloudIdentity {
    /// Workspace-only credentials are admitted by CloudApi solely for loopback
    /// acceptance servers. They must never stand in for a production account.
    pub fn workspace(workspace: Auth) -> Self {
        Self {
            account: None,
            workspace,
            hosted_scope: None,
        }
    }
    pub fn with_account(mut self, account: Option<Account>) -> Self {
        self.account = account;
        self
    }
    pub(crate) fn hosted(
        account: Account,
        scope: watch::Receiver<(Option<String>, Option<String>)>,
    ) -> Self {
        let mut identity = Self::workspace(Auth::new(crate::auth::AuthConfig::new(
            "http://127.0.0.1:1",
            std::path::PathBuf::new(),
        )))
        .with_account(Some(account));
        identity.hosted_scope = Some(scope);
        identity
    }
    pub fn workos_enabled(&self) -> bool {
        self.account.is_some()
    }
    pub fn state(&self) -> AuthState {
        match &self.account {
            Some(account) => match account.status().user {
                Some(user) => {
                    let org_id = match &self.hosted_scope {
                        Some(scope) => {
                            let scope = scope.borrow();
                            if scope.0.as_deref() == Some(user.id.as_str()) {
                                scope.1.clone()
                            } else {
                                None
                            }
                        }
                        None => organization(&self.workspace.state(), &user.id),
                    };
                    AuthState::SignedIn {
                        user: AuthUser {
                            id: user.id,
                            email: user.email,
                            name: user.name,
                        },
                        org_id,
                    }
                }
                None => AuthState::SignedOut,
            },
            None => self.workspace.state(),
        }
    }
    pub fn user_id(&self) -> Option<String> {
        self.state().user().map(|user| user.id.clone())
    }
    pub async fn access_token(&self) -> anyhow::Result<String> {
        match &self.account {
            Some(account) => {
                let owner = self.user_id();
                let credential = account.credential().await?;
                anyhow::ensure!(
                    owner.as_deref() == Some(&credential.subject) && self.user_id() == owner,
                    "Your account changed. Reopen this view."
                );
                Ok(credential.access_token)
            }
            None => Ok(self.workspace.access_token().await?),
        }
    }
    pub fn profile_dir(&self) -> &Path {
        self.workspace.profile_dir()
    }
    pub fn watch_state(&self) -> Changes {
        Changes {
            account: self.account.as_ref().map(Account::watch),
            workspace: self.workspace.watch_state(),
            scope: self.hosted_scope.clone(),
        }
    }
    pub fn status_stream(&self) -> futures::stream::BoxStream<'static, serde_json::Value> {
        use futures::StreamExt;
        let identity = self.clone();
        let changes = self.watch_state();
        futures::stream::unfold(
            (identity, changes, true),
            |(identity, mut changes, first)| async move {
                if !first {
                    changes.changed().await.ok()?;
                }
                let mut value = serde_json::to_value(identity.account.as_ref()?.status()).ok()?;
                value["orgId"] = serde_json::json!(identity.state().org_id());
                Some((value, (identity, changes, false)))
            },
        )
        .boxed()
    }
    #[cfg(test)]
    pub fn new(config: crate::auth::AuthConfig) -> Self {
        Self::workspace(Auth::new(config))
    }
    #[cfg(test)]
    pub fn sign_out(&self) {
        match &self.account {
            Some(account) => {
                account.sign_out().unwrap();
            }
            None => self.workspace.sign_out(),
        }
    }
}
fn organization(workspace: &AuthState, subject: &str) -> Option<String> {
    match workspace {
        AuthState::SignedIn { user, org_id } if user.id == subject => org_id.clone(),
        _ => None,
    }
}
pub struct Changes {
    scope: Option<watch::Receiver<(Option<String>, Option<String>)>>,
    account: Option<watch::Receiver<crate::account::AccountStatus>>,
    workspace: watch::Receiver<AuthState>,
}
impl Changes {
    pub async fn changed(&mut self) -> Result<(), watch::error::RecvError> {
        match (&mut self.account, &mut self.scope) {
            (Some(account), Some(scope)) => {
                tokio::select! { result = account.changed() => result, result = self.workspace.changed() => result, result = scope.changed() => result }
            }
            (Some(account), None) => {
                tokio::select! { result = account.changed() => result, result = self.workspace.changed() => result }
            }
            (None, Some(scope)) => {
                tokio::select! { result = self.workspace.changed() => result, result = scope.changed() => result }
            }
            (None, None) => self.workspace.changed().await,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn organization_scope_requires_the_same_general_account() {
        let state = AuthState::SignedIn {
            user: AuthUser {
                id: "member".into(),
                email: "member@example.test".into(),
                name: None,
            },
            org_id: Some("team".into()),
        };
        assert_eq!(organization(&state, "member").as_deref(), Some("team"));
        assert_eq!(organization(&state, "other"), None);
        assert_eq!(organization(&AuthState::SignedOut, "member"), None);
    }
}
