//! Transient broker primitive. Never expose this service to generic shell tools.
//! Values deliberately implement neither Debug nor Serialize.
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Scope {
    pub project: String,
    pub session: String,
    pub run: String,
    pub operation: String,
}
struct Entry {
    scope: Scope,
    deadline: Instant,
    value: Option<Zeroizing<String>>,
    route: Option<(String, String, String)>,
}
#[derive(Default)]
pub(crate) struct Broker {
    entries: HashMap<String, Entry>,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Unavailable,
    InvalidValue,
    AlreadyAnswered,
}
impl Broker {
    pub fn request(&mut self, scope: Scope, ttl: Duration) -> Result<String, Error> {
        self.sweep();
        if ttl.is_zero() || ttl > Duration::from_secs(3600) || self.entries.len() >= 128 {
            return Err(Error::Unavailable);
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.entries.insert(
            id.clone(),
            Entry {
                scope,
                deadline: Instant::now() + ttl,
                value: None,
                route: None,
            },
        );
        Ok(id)
    }
    // Caller must authenticate the human and authorize the scope before invoking.
    pub fn submit(
        &mut self,
        id: &str,
        scope: &Scope,
        value: Zeroizing<String>,
    ) -> Result<(), Error> {
        self.sweep();
        let entry = self
            .entries
            .get_mut(id)
            .filter(|e| &e.scope == scope)
            .ok_or(Error::Unavailable)?;
        if entry.value.is_some() {
            return Err(Error::AlreadyAnswered);
        }
        if value.is_empty() || value.len() > 65536 {
            return Err(Error::InvalidValue);
        }
        entry.value = Some(value);
        Ok(())
    }
    // Only a trusted fixed backend operation may receive this value. No RPC or
    // tool result may serialize it. Removing before execution makes failures one-use.
    pub fn consume(&mut self, id: &str, scope: &Scope) -> Result<Zeroizing<String>, Error> {
        self.sweep();
        let entry = self
            .entries
            .get(id)
            .filter(|e| &e.scope == scope && e.value.is_some())
            .ok_or(Error::Unavailable)?;
        let _ = entry;
        self.entries
            .remove(id)
            .and_then(|e| e.value)
            .ok_or(Error::Unavailable)
    }
    #[cfg(test)]
    pub fn revoke_run(&mut self, project: &str, session: &str, run: &str) {
        self.entries.retain(|_, e| {
            !(e.scope.project == project && e.scope.session == session && e.scope.run == run)
        });
    }
    pub fn sweep(&mut self) {
        let now = Instant::now();
        self.entries.retain(|_, e| e.deadline > now);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn scope() -> Scope {
        Scope {
            project: "p".into(),
            session: "s".into(),
            run: "r".into(),
            operation: "op".into(),
        }
    }
    #[test]
    fn scoped_one_use_and_no_overwrite() {
        let mut broker = Broker::default();
        let s = scope();
        let id = broker.request(s.clone(), Duration::from_secs(60)).unwrap();
        broker
            .submit(&id, &s, Zeroizing::new("sentinel".into()))
            .unwrap();
        assert_eq!(
            broker.submit(&id, &s, Zeroizing::new("replacement".into())),
            Err(Error::AlreadyAnswered)
        );
        let mut wrong = s.clone();
        wrong.operation = "other".into();
        assert!(matches!(
            broker.consume(&id, &wrong),
            Err(Error::Unavailable)
        ));
        assert_eq!(broker.consume(&id, &s).unwrap().as_str(), "sentinel");
        assert!(matches!(broker.consume(&id, &s), Err(Error::Unavailable)));
    }
    #[test]
    fn expiry_and_revocation() {
        let mut broker = Broker::default();
        let s = scope();
        let id = broker.request(s.clone(), Duration::from_secs(60)).unwrap();
        broker.entries.get_mut(&id).unwrap().deadline = Instant::now();
        assert_eq!(
            broker.submit(&id, &s, Zeroizing::new("x".into())),
            Err(Error::Unavailable)
        );
        let id = broker.request(s.clone(), Duration::from_secs(60)).unwrap();
        broker.revoke_run("p", "s", "r");
        assert!(matches!(broker.consume(&id, &s), Err(Error::Unavailable)));
    }
}

// The broker is process-local and never serialized or written to disk.
pub(crate) fn shared() -> std::sync::MutexGuard<'static, Broker> {
    static BROKER: std::sync::OnceLock<std::sync::Mutex<Broker>> = std::sync::OnceLock::new();
    BROKER
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
impl Broker {
    pub fn bind(&mut self, namespace: &str, id: &str, chat: &str, request: &str) {
        if let Some(entry) = self.entries.get_mut(id) {
            if entry.route.is_none() {
                entry.route = Some((namespace.into(), chat.into(), request.into()));
            }
        }
    }
    pub fn submit_routed(
        &mut self,
        namespace: &str,
        id: &str,
        chat: &str,
        request: &str,
        value: Zeroizing<String>,
    ) -> Result<(), Error> {
        self.sweep();
        let scope = self
            .entries
            .get(id)
            .filter(|e| {
                e.route
                    .as_ref()
                    .is_some_and(|(n, c, r)| n == namespace && c == chat && r == request)
            })
            .map(|e| e.scope.clone())
            .ok_or(Error::Unavailable)?;
        self.submit(id, &scope, value)
    }
    pub fn decline_routed(
        &mut self,
        namespace: &str,
        id: &str,
        chat: &str,
        request: &str,
    ) -> Result<(), Error> {
        self.sweep();
        if !self.entries.get(id).is_some_and(|e| {
            e.route
                .as_ref()
                .is_some_and(|(n, c, r)| n == namespace && c == chat && r == request)
        }) {
            return Err(Error::Unavailable);
        }
        self.entries.remove(id);
        Ok(())
    }
    pub fn remove(&mut self, id: &str) {
        self.entries.remove(id);
    }
}
pub(crate) struct Lease(pub String);
impl Drop for Lease {
    fn drop(&mut self) {
        shared().remove(&self.0);
    }
}

// Conservative destination policy; mapped IPv4 addresses follow IPv4 policy.
pub(crate) fn public_address(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_documentation()
                || a == 0
                || a >= 240
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192 && b == 0)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 169 && b == 254)
                || (a == 192 && b == 88 && c == 99))
        }
        std::net::IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_address(v4.into());
            }
            // Only global unicast, excluding documentation and special-purpose allocations.
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x200 || s[1] == 0xdb8))
                && !(s[0] == 0x2002)
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

pub(crate) async fn execute<F, Fut>(
    input: serde_json::Value,
    context: zuse_harness::ToolContext,
    ask: F,
) -> Result<String, zuse_harness::Error>
where
    F: FnOnce(zuse_protocol::UserInputQuestion) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<String>, zuse_harness::Error>>,
{
    use zuse_harness::Error;
    use zuse_protocol::UserInputQuestion;

    use crate::secret_broker::{self, Scope};
    if context.read_only
        || !matches!(
            input["method"].as_str(),
            Some("GET" | "POST" | "PUT" | "PATCH" | "DELETE")
        )
        || ![
            ("label", 1, 200),
            ("reason", 1, 2000),
            ("url", 1, 2048),
            ("body", 0, 16000),
        ]
        .iter()
        .all(|(key, min, max)| {
            input
                .get(*key)
                .and_then(|v| v.as_str())
                .map(|s| s.len() >= *min && s.len() <= *max)
                .unwrap_or(*key == "body")
        })
    {
        return Err(Error::Tool("Invalid private operation".into()));
    }
    let fail = || Error::Tool("Private secret operation failed".into());
    let url = reqwest::Url::parse(input["url"].as_str().ok_or_else(fail)?).map_err(|_| fail())?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
        || url.host_str().is_some_and(|h| {
            h == "localhost" || h.ends_with(".localhost") || h.parse::<std::net::IpAddr>().is_ok()
        })
    {
        return Err(Error::Tool(
            "Use a public HTTPS URL without credentials or fragments".into(),
        ));
    }
    let method = reqwest::Method::from_bytes(input["method"].as_str().ok_or_else(fail)?.as_bytes())
        .map_err(|_| fail())?;
    let body = input["body"].as_str().unwrap_or_default().to_owned();
    let scope = Scope {
        project: uuid::Uuid::new_v4().to_string(),
        session: context.agent_id.clone(),
        run: context.call_id.clone(),
        operation: format!("{method} {url}"),
    };
    let id = secret_broker::shared()
        .request(scope.clone(), std::time::Duration::from_secs(600))
        .map_err(|_| fail())?;
    let _lease = secret_broker::Lease(id.clone());
    let question_id = format!("private-secret:{id}");
    let question = UserInputQuestion {
        approval_prompt: None,
        approval: None,
        id: question_id,
        header: "Private secret".into(),
        question: format!(
            "{}\n{}\n\nApprove one request: {} {}\nBody: {}\n\nBearer token is used once by the backend. Only HTTP status is returned. Expires in 10 minutes. No file or shell access.",
            input["label"].as_str().unwrap_or("Token"),
            input["reason"].as_str().unwrap_or_default(),
            method,
            url,
            body
        ),
        options: vec![],
        multi_select: false,
        prefill: None,
        multiline: false,
    };
    let answer = tokio::time::timeout(std::time::Duration::from_secs(600), ask(question))
        .await
        .map_err(|_| fail())??;
    if answer != ["saved"] {
        return Ok("Secret request declined".into());
    }
    if context.cancel.is_cancelled() {
        return Err(Error::Interrupted);
    }
    let token = secret_broker::shared()
        .consume(&id, &scope)
        .map_err(|_| fail())?;
    let bearer = Zeroizing::new(format!("Bearer {}", token.as_str()));
    let mut auth = reqwest::header::HeaderValue::from_str(bearer.as_str()).map_err(|_| fail())?;
    auth.set_sensitive(true);
    // Resolve once, reject local/private ranges, and pin the validated addresses.
    // This prevents DNS rebinding from turning an approved public URL into an
    // authenticated request to a local service or cloud metadata endpoint.
    let host = url.host_str().ok_or_else(fail)?.to_owned();
    let port = url.port_or_known_default().ok_or_else(fail)?;
    let addresses: Vec<_> = tokio::select! {
        biased;
        _ = context.cancel.cancelled() => return Err(Error::Interrupted),
        result = tokio::time::timeout(std::time::Duration::from_secs(10), tokio::net::lookup_host((host.as_str(), port))) => result.map_err(|_| fail())?.map_err(|_| fail())?.collect(),
    };
    if addresses.is_empty()
        || addresses
            .iter()
            .any(|a| !secret_broker::public_address(a.ip()))
    {
        return Err(Error::Tool(
            "Private secret operations require a public HTTPS destination".into(),
        ));
    }
    let client = reqwest::Client::builder()
        .resolve_to_addrs(&host, &addresses)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| fail())?;
    let request = client
        .request(method, url)
        .header(reqwest::header::AUTHORIZATION, auth)
        .header(reqwest::header::USER_AGENT, "Zuse-private-operation")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body);
    let response = tokio::select! {
        biased;
        _ = context.cancel.cancelled() => return Err(Error::Interrupted),
        result = request.send() => result.map_err(|_| fail())?,
    };
    Ok(format!(
        "Private operation completed: HTTP {}. Response body withheld.",
        response.status().as_u16()
    ))
}

#[cfg(test)]
mod operation_tests {
    use super::*;
    use serde_json::json;
    fn context() -> zuse_harness::ToolContext {
        zuse_harness::ToolContext {
            agent_id: "agent".into(),
            call_id: "call".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
            permission_mode: zuse_harness::PermissionMode::Default,
            read_only: false,
        }
    }
    fn input() -> serde_json::Value {
        json!({"label":"API token", "reason":"Check access", "url":"https://api.github.com/user", "method":"GET"})
    }
    #[test]
    fn destinations_reject_internal_and_special_ranges() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "100.64.0.1",
            "192.168.1.1",
            "198.18.0.1",
            "224.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "2001:db8::1",
            "2002:7f00:1::",
        ] {
            assert!(!public_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_address(ip.parse().unwrap()));
        }
    }
    #[test]
    fn routing_is_bound_and_invalid_submission_does_not_consume() {
        let mut b = Broker::default();
        let scope = Scope {
            project: "p".into(),
            session: "s".into(),
            run: "r".into(),
            operation: "op".into(),
        };
        let id = b.request(scope.clone(), Duration::from_secs(60)).unwrap();
        b.bind("engine", &id, "chat", "request");
        b.bind("engine", &id, "attacker", "request");
        assert_eq!(
            b.submit_routed(
                "different-profile",
                &id,
                "chat",
                "request",
                Zeroizing::new("sentinel".into())
            ),
            Err(Error::Unavailable)
        );
        assert_eq!(
            b.decline_routed("different-profile", &id, "chat", "request"),
            Err(Error::Unavailable)
        );
        assert_eq!(
            b.submit_routed(
                "engine",
                &id,
                "attacker",
                "request",
                Zeroizing::new("sentinel".into())
            ),
            Err(Error::Unavailable)
        );
        assert_eq!(
            b.decline_routed("engine", &id, "chat", "wrong"),
            Err(Error::Unavailable)
        );
        assert_eq!(
            b.submit_routed(
                "engine",
                &id,
                "chat",
                "request",
                Zeroizing::new(String::new())
            ),
            Err(Error::InvalidValue)
        );
        b.submit_routed(
            "engine",
            &id,
            "chat",
            "request",
            Zeroizing::new("sentinel".into()),
        )
        .unwrap();
        assert_eq!(b.consume(&id, &scope).unwrap().as_str(), "sentinel");
    }
    #[tokio::test]
    async fn decline_and_cancel_remove_values() {
        let result = execute(input(), context(), |q| async move {
            let id = q.id.strip_prefix("private-secret:").unwrap();
            assert!(shared().entries.contains_key(id));
            Ok(vec!["declined".into()])
        })
        .await
        .unwrap();
        assert_eq!(result, "Secret request declined");
        let mut captured = None;
        let result = execute(input(), context(), |q| {
            captured = Some(q.id.strip_prefix("private-secret:").unwrap().to_owned());
            std::future::ready(Err(zuse_harness::Error::Interrupted))
        })
        .await;
        assert!(matches!(result, Err(zuse_harness::Error::Interrupted)));
        assert!(!shared().entries.contains_key(&captured.unwrap()));
    }
    #[tokio::test]
    async fn errors_never_return_submitted_secret() {
        let mut captured = None;
        let result = execute(input(), context(), |q| {
            let id = q.id.strip_prefix("private-secret:").unwrap().to_owned();
            let mut broker = shared();
            let scope = broker.entries[&id].scope.clone();
            broker
                .submit(
                    &id,
                    &scope,
                    Zeroizing::new("sentinel-private-token\ninvalid-header".into()),
                )
                .unwrap();
            captured = Some(id);
            std::future::ready(Ok(vec!["saved".into()]))
        })
        .await;
        let error = result.unwrap_err().to_string();
        assert!(!error.contains("sentinel"));
        assert!(!shared().entries.contains_key(&captured.unwrap()));
    }
    #[tokio::test]
    async fn invalid_or_read_only_operations_never_prompt() {
        let mut ctx = context();
        ctx.read_only = true;
        assert!(
            execute(input(), ctx, |_| async { panic!("must not prompt") })
                .await
                .is_err()
        );
        for url in [
            "http://api.github.com/user",
            "https://user:password@api.github.com/user",
            "https://127.0.0.1/",
        ] {
            let mut i = input();
            i["url"] = json!(url);
            assert!(
                execute(i, context(), |_| async { panic!("must not prompt") })
                    .await
                    .is_err()
            );
        }
    }
}
