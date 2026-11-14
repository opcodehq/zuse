//! Who may use cloud writes.
//!
//! `ZUSE_CLOUD_ACCESS_POLICY=allowlist` (the default) keeps every per-feature
//! account list (`ZUSE_CLOUD_PROJECT_WRITE_ACCOUNTS`, `ZUSE_CLOUD_CREATE_ACCOUNTS`,
//! ...) authoritative: an empty list turns that feature off for everyone.
//!
//! `ZUSE_CLOUD_ACCESS_POLICY=entitled` opens those lists to every signed-in
//! account. The paid `cloud-workspace` entitlement and billing capacity checks,
//! which already guard every path that allocates billable compute, become the
//! real gate. Staging-only mechanisms (staging base pin, staging compute
//! allowance, staging pilots) refuse to run under this policy.
use std::{collections::BTreeSet, sync::Arc};

pub const ENV: &str = "ZUSE_CLOUD_ACCESS_POLICY";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AccessPolicy {
    #[default]
    Allowlist,
    Entitled,
}

impl AccessPolicy {
    pub fn parse(raw: Option<&str>) -> anyhow::Result<Self> {
        match raw.map(str::trim) {
            None | Some("") | Some("allowlist") => Ok(Self::Allowlist),
            Some("entitled") => Ok(Self::Entitled),
            Some(_) => anyhow::bail!("{ENV} must be allowlist or entitled"),
        }
    }
    pub fn from_env() -> anyhow::Result<Self> {
        Self::parse(std::env::var(ENV).ok().as_deref())
    }
    /// The effective account set for one feature's explicit list.
    pub fn accounts(self, listed: impl IntoIterator<Item = String>) -> Accounts {
        match self {
            Self::Allowlist => Accounts::listed(listed),
            Self::Entitled => Accounts::open(),
        }
    }
    pub fn accounts_from_env(self, var: &str) -> Accounts {
        self.accounts(listed_from_env(var))
    }
}

/// Comma separated account IDs from one environment variable.
pub fn listed_from_env(var: &str) -> Vec<String> {
    std::env::var(var)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Accounts allowed through one feature gate: an explicit list, or every account.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Accounts {
    open: bool,
    listed: Arc<BTreeSet<String>>,
}

impl Accounts {
    pub fn open() -> Self {
        Self {
            open: true,
            listed: Arc::default(),
        }
    }
    pub fn listed(accounts: impl IntoIterator<Item = String>) -> Self {
        Self {
            open: false,
            listed: Arc::new(accounts.into_iter().filter(|a| !a.is_empty()).collect()),
        }
    }
    /// Read `var` under the deployment's `ZUSE_CLOUD_ACCESS_POLICY`.
    pub fn from_env(var: &str) -> anyhow::Result<Self> {
        Ok(AccessPolicy::from_env()?.accounts_from_env(var))
    }
    pub fn contains(&self, account: &str) -> bool {
        if self.open {
            !account.is_empty()
        } else {
            self.listed.contains(account)
        }
    }
    /// Nobody may use the feature.
    pub fn is_empty(&self) -> bool {
        !self.open && self.listed.is_empty()
    }
    pub fn is_open(&self) -> bool {
        self.open
    }
    /// The one listed account, for operator commands that act on exactly one owner.
    pub fn single(&self) -> Option<&str> {
        if self.open || self.listed.len() != 1 {
            return None;
        }
        self.listed.iter().next().map(String::as_str)
    }
    /// Bind as `$n::text[]` with `($n::text[] IS NULL OR column=ANY($n))`:
    /// `None` means every account.
    pub fn filter(&self) -> Option<Vec<String>> {
        (!self.open).then(|| self.listed.iter().cloned().collect())
    }
}

impl From<Vec<String>> for Accounts {
    fn from(accounts: Vec<String>) -> Self {
        Self::listed(accounts)
    }
}

/// Under the entitled policy a first-time account has no migration-era owner
/// markers yet; mark it Rust-owned so the existing `backend='rust'` fences
/// accept it. An explicit `legacy` marker is never overwritten.
pub(crate) async fn claim_rust_owner(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    accounts: &Accounts,
    account: &str,
    kinds: &[&str],
    now: i64,
) -> Result<(), crate::error::ApiError> {
    if !accounts.is_open() || account.is_empty() {
        return Ok(());
    }
    sqlx::query("INSERT INTO zuse_rust_control_owners(resource_kind,resource_id,backend,generation,updated_at) SELECT kind,$1,'rust',1,$3 FROM unnest($2::text[]) kind ON CONFLICT(resource_kind,resource_id) DO NOTHING")
        .bind(account)
        .bind(kinds.iter().map(|k| k.to_string()).collect::<Vec<_>>())
        .bind(now)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_is_allowlist_and_unknown_values_fail() {
        assert_eq!(AccessPolicy::parse(None).unwrap(), AccessPolicy::Allowlist);
        assert_eq!(
            AccessPolicy::parse(Some("")).unwrap(),
            AccessPolicy::Allowlist
        );
        assert_eq!(
            AccessPolicy::parse(Some("allowlist")).unwrap(),
            AccessPolicy::Allowlist
        );
        assert_eq!(
            AccessPolicy::parse(Some(" entitled ")).unwrap(),
            AccessPolicy::Entitled
        );
        assert!(AccessPolicy::parse(Some("open")).is_err());
        assert!(AccessPolicy::parse(Some("Entitled")).is_err());
    }

    #[test]
    fn allowlist_keeps_empty_lists_closed() {
        let none = AccessPolicy::Allowlist.accounts(Vec::<String>::new());
        assert!(none.is_empty());
        assert!(!none.contains("user_a"));
        assert_eq!(none.filter(), Some(vec![]));
        let one = AccessPolicy::Allowlist.accounts(vec!["user_a".to_owned()]);
        assert!(one.contains("user_a"));
        assert!(!one.contains("user_b"));
        assert_eq!(one.single(), Some("user_a"));
        assert_eq!(one.filter(), Some(vec!["user_a".to_owned()]));
    }

    #[test]
    fn entitled_accepts_every_signed_in_account() {
        let open = AccessPolicy::Entitled.accounts(Vec::<String>::new());
        assert!(!open.is_empty());
        assert!(open.contains("user_unlisted"));
        assert!(open.contains("organization:org_1"));
        assert!(!open.contains(""));
        assert_eq!(open.single(), None);
        assert_eq!(open.filter(), None);
    }
}
