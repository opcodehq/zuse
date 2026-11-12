//! Terminal controls for pairing and account access on a running engine.
use crate::Command;
use anyhow::Context;
use serde_json::{Value, json};
use std::io::IsTerminal;
use zuse_i18n::{Localizer, messages::cli as m};
use zuse_remote::{Route, RouteKind, routes::classify_origin};
use zuse_rpc::{RpcClient, connect_ws, methods};

// Isolating controls must not enter copyable terminal links.
pub(crate) fn plain(text: String) -> String {
    text.replace(['\u{2068}', '\u{2069}'], "")
}

pub(crate) async fn run(command: Command, port: u16, l10n: &Localizer) -> anyhow::Result<()> {
    let client = connect_ws(&format!("ws://127.0.0.1:{port}"))
        .await
        .map_err(|_| anyhow::anyhow!(plain(m::server_unavailable(l10n, port.to_string()))))?;
    match command {
        Command::Mcp => unreachable!("handled before connecting"),
        Command::Pair { label, app } => invite(&client, label, app, l10n).await,
        Command::Login => sign_in(&client, l10n).await,
        Command::Logout => {
            client
                .call(methods::CLOUD_ACCOUNT_SIGN_OUT, json!({}))
                .await?;
            println!("{}", m::signed_out(l10n));
            Ok(())
        }
        Command::Link { name } => link(&client, name, l10n).await,
        Command::Unlink => {
            client.call(methods::UNLINK_MANAGED_HOST, json!({})).await?;
            println!("{}", m::unlinked(l10n));
            Ok(())
        }
        Command::Status => status(&client, l10n).await,
    }
}

/// Create a private invitation and show the ways to open it.
pub(crate) async fn invite(
    client: &RpcClient,
    label: Option<String>,
    prefer_app: bool,
    l10n: &Localizer,
) -> anyhow::Result<()> {
    let label = label.unwrap_or_else(|| m::default_device(l10n));
    let reply = client
        .call(methods::CREATE_PAIRING, json!({"label":label}))
        .await?;
    let links = Links::from_reply(&reply).context(m::invitation_failed(l10n))?;
    if let Some(browser) = &links.browser {
        println!("{}", plain(m::browser_link(l10n, browser.as_str())));
    }
    if let Some(app) = &links.app {
        println!("{}", plain(m::app_link(l10n, app.as_str())));
    }
    if std::io::stdout().is_terminal()
        && let Some(code) = links.qr_target(prefer_app).and_then(qr_code)
    {
        println!("{code}");
    }
    if links.self_signed {
        println!("{}", m::certificate_warning(l10n));
    }
    println!(
        "{}",
        if links.local_only {
            m::local_only(l10n)
        } else {
            m::link_warning(l10n)
        }
    );
    Ok(())
}

#[derive(Debug, PartialEq)]
struct Links {
    browser: Option<String>,
    app: Option<String>,
    self_signed: bool,
    local_only: bool,
}
impl Links {
    fn from_reply(reply: &Value) -> Option<Self> {
        let fragment = reply["fragment"].as_str()?;
        let link = |key: &str| {
            reply[key]
                .as_str()
                .map(|origin| format!("{origin}/{fragment}"))
        };
        let origin = reply["origin"].as_str();
        let local_only = origin.is_none_or(|origin| classify_origin(origin) == RouteKind::Loopback);
        // A loopback address is secure enough for a browser on this computer.
        let browser = link("browserOrigin").or_else(|| link("origin").filter(|_| local_only));
        let app = link("origin").filter(|app| Some(app) != browser.as_ref());
        Some(Self {
            browser,
            app,
            self_signed: reply["browserSelfSigned"] == true,
            local_only,
        })
    }
    fn qr_target(&self, prefer_app: bool) -> Option<&str> {
        let (first, second) = if prefer_app {
            (&self.app, &self.browser)
        } else {
            (&self.browser, &self.app)
        };
        first.as_deref().or(second.as_deref())
    }
}

fn qr_code(link: &str) -> Option<String> {
    use qrcode::render::unicode::Dense1x2;
    let image = qrcode::QrCode::new(link)
        .ok()?
        .render::<Dense1x2>()
        .dark_color(Dense1x2::Light)
        .light_color(Dense1x2::Dark)
        .build();
    // Fixed colors keep the code scannable on light terminal themes too.
    Some(
        image
            .lines()
            .map(|line| format!("\x1b[97;40m{line}\x1b[0m"))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub(crate) async fn sign_in(client: &RpcClient, l10n: &Localizer) -> anyhow::Result<()> {
    let mut updates = client
        .subscribe(methods::CLOUD_ACCOUNT_STATUS, json!({}))
        .await?;
    let first = updates.recv().await.context(m::sign_in_failed(l10n))?;
    if let Some(email) = first["user"]["email"].as_str() {
        println!("{}", plain(m::signed_in(l10n, email)));
        return Ok(());
    }
    let progress = client
        .call(methods::CLOUD_ACCOUNT_SIGN_IN, json!({}))
        .await?;
    println!(
        "{}",
        plain(m::sign_in_prompt(
            l10n,
            progress["userCode"].as_str().unwrap_or_default(),
            progress["url"].as_str().unwrap_or_default(),
        ))
    );
    let mut waiting = false;
    while let Some(status) = updates.recv().await {
        if let Some(email) = status["user"]["email"].as_str() {
            println!("{}", plain(m::signed_in(l10n, email)));
            return Ok(());
        }
        waiting |= status["login"].is_object();
        if status["login"].is_null() && (waiting || status["error"].is_string()) {
            break;
        }
    }
    anyhow::bail!(m::sign_in_failed(l10n))
}

pub(crate) async fn link(
    client: &RpcClient,
    name: Option<String>,
    l10n: &Localizer,
) -> anyhow::Result<()> {
    let mut status = client.call(methods::MANAGED_HOST_STATUS, json!({})).await?;
    if status["configured"] != true {
        let label =
            name.unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned());
        status = client
            .call(methods::LINK_MANAGED_HOST, json!({"label":label}))
            .await?;
    }
    println!(
        "{}",
        plain(m::linked(
            l10n,
            status["label"].as_str().unwrap_or_default(),
            status["endpoint"]["httpBaseUrl"]
                .as_str()
                .unwrap_or_default(),
        ))
    );
    Ok(())
}

async fn status(client: &RpcClient, l10n: &Localizer) -> anyhow::Result<()> {
    let account = client
        .subscribe(methods::CLOUD_ACCOUNT_STATUS, json!({}))
        .await?
        .recv()
        .await
        .unwrap_or_default();
    match account["user"]["email"].as_str() {
        Some(email) => println!("{}", plain(m::status_signed_in(l10n, email))),
        None => println!("{}", m::status_signed_out(l10n)),
    }
    let host = client.call(methods::MANAGED_HOST_STATUS, json!({})).await?;
    if host["configured"] == true {
        println!(
            "{}",
            plain(m::status_linked(
                l10n,
                host["label"].as_str().unwrap_or_default(),
                host["endpoint"]["httpBaseUrl"].as_str().unwrap_or_default(),
            ))
        );
    } else {
        println!("{}", m::status_unlinked(l10n));
    }
    let direct = client
        .call(methods::DIRECT_ACCESS_STATUS, json!({}))
        .await?;
    let routes: Vec<Route> = serde_json::from_value(direct["routes"].clone()).unwrap_or_default();
    if routes.is_empty() {
        println!("{}", m::status_no_routes(l10n));
    }
    for route in routes {
        println!("{}: {}", route.kind.label(), route.origin);
    }
    if let Some(origin) = direct["browserOrigin"].as_str() {
        println!("{}", plain(m::browser_link(l10n, origin)));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links(value: Value) -> Links {
        Links::from_reply(&value).unwrap()
    }

    #[test]
    fn lan_invitations_offer_https_to_browsers_and_plain_routes_to_apps() {
        let links = links(json!({
            "fragment":"#pair=abc",
            "origin":"http://192.168.1.20:27657",
            "browserOrigin":"https://192.168.1.20:27659",
            "browserSelfSigned":true,
        }));
        assert_eq!(
            links,
            Links {
                browser: Some("https://192.168.1.20:27659/#pair=abc".into()),
                app: Some("http://192.168.1.20:27657/#pair=abc".into()),
                self_signed: true,
                local_only: false,
            }
        );
        assert_eq!(links.qr_target(false), links.browser.as_deref());
        assert_eq!(links.qr_target(true), links.app.as_deref());
    }

    #[test]
    fn one_public_address_serves_browsers_and_apps_alike() {
        let links = links(json!({
            "fragment":"#pair=abc",
            "origin":"https://box.example",
            "browserOrigin":"https://box.example",
            "browserSelfSigned":false,
        }));
        assert_eq!(
            links.browser.as_deref(),
            Some("https://box.example/#pair=abc")
        );
        assert_eq!(links.app, None);
        assert_eq!(links.qr_target(true), links.browser.as_deref());
    }

    #[test]
    fn loopback_invitations_say_they_only_work_here() {
        let links = links(json!({
            "fragment":"#pair=abc",
            "origin":"http://127.0.0.1:27656",
            "browserOrigin":null,
            "browserSelfSigned":false,
        }));
        assert!(links.local_only);
        assert_eq!(
            links.browser.as_deref(),
            Some("http://127.0.0.1:27656/#pair=abc")
        );
        assert_eq!(links.app, None);
    }

    #[test]
    fn plain_app_routes_are_kept_when_no_browser_route_exists() {
        let links = links(json!({
            "fragment":"#pair=abc",
            "origin":"http://192.168.1.20:27657",
            "browserOrigin":null,
            "browserSelfSigned":false,
        }));
        assert_eq!(links.browser, None);
        assert!(!links.local_only);
        assert_eq!(links.qr_target(false), links.app.as_deref());
    }

    struct Account {
        updates: Vec<Value>,
        host: Value,
        requests: std::sync::Mutex<Vec<(String, Value)>>,
    }
    #[async_trait::async_trait]
    impl zuse_rpc::RpcService for Account {
        async fn handle(
            &self,
            method: &str,
            params: Value,
        ) -> Result<zuse_rpc::RpcReply, zuse_rpc::RpcError> {
            use futures::StreamExt;
            self.requests.lock().unwrap().push((method.into(), params));
            match method {
                methods::CLOUD_ACCOUNT_STATUS => Ok(zuse_rpc::RpcReply::Stream(
                    futures::stream::iter(self.updates.clone()).boxed(),
                )),
                methods::CLOUD_ACCOUNT_SIGN_IN => {
                    zuse_rpc::RpcReply::value(&json!({"url":"https://sign.in","userCode":"ABCD"}))
                }
                methods::MANAGED_HOST_STATUS => zuse_rpc::RpcReply::value(&self.host),
                methods::LINK_MANAGED_HOST => zuse_rpc::RpcReply::value(&json!({
                    "configured":true,
                    "label":"laptop",
                    "endpoint":{"httpBaseUrl":"https://laptop.example"},
                })),
                _ => unreachable!(),
            }
        }
    }
    fn account(updates: Vec<Value>, host: Value) -> (std::sync::Arc<Account>, RpcClient) {
        let service = std::sync::Arc::new(Account {
            updates,
            host,
            requests: Default::default(),
        });
        let client = zuse_rpc::memory_client(service.clone());
        (service, client)
    }
    fn called(service: &Account, method: &str) -> bool {
        service
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|(name, _)| name == method)
    }
    const USER: &str = "person@example.com";

    #[tokio::test]
    async fn signing_in_is_skipped_when_the_account_is_already_there() {
        let (service, client) = account(vec![json!({"user":{"email":USER}})], Value::Null);
        sign_in(&client, &Localizer::new(zuse_i18n::Locale::En))
            .await
            .unwrap();
        assert!(!called(&service, methods::CLOUD_ACCOUNT_SIGN_IN));
    }

    #[tokio::test]
    async fn signing_in_waits_for_the_code_to_be_confirmed() {
        let (service, client) = account(
            vec![
                json!({"user":null,"login":null}),
                json!({"user":null,"login":{"url":"https://sign.in"}}),
                json!({"user":{"email":USER},"login":null}),
            ],
            Value::Null,
        );
        sign_in(&client, &Localizer::new(zuse_i18n::Locale::En))
            .await
            .unwrap();
        assert!(called(&service, methods::CLOUD_ACCOUNT_SIGN_IN));
    }

    #[tokio::test]
    async fn an_expired_code_ends_sign_in_with_an_error() {
        let (_, client) = account(
            vec![
                json!({"user":null,"login":null}),
                json!({"user":null,"login":{"url":"https://sign.in"}}),
                json!({"user":null,"login":null,"error":"Expired"}),
            ],
            Value::Null,
        );
        assert!(
            sign_in(&client, &Localizer::new(zuse_i18n::Locale::En))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn linking_keeps_an_existing_link_and_names_new_ones() {
        let l10n = Localizer::new(zuse_i18n::Locale::En);
        let (service, client) = account(vec![], json!({"configured":false}));
        link(&client, Some("laptop".into()), &l10n).await.unwrap();
        let requests = service.requests.lock().unwrap().clone();
        assert_eq!(requests[1].0, methods::LINK_MANAGED_HOST);
        assert_eq!(requests[1].1["label"], "laptop");
        let (service, client) = account(
            vec![],
            json!({"configured":true,"label":"old","endpoint":{"httpBaseUrl":"https://old.example"}}),
        );
        link(&client, Some("laptop".into()), &l10n).await.unwrap();
        assert!(!called(&service, methods::LINK_MANAGED_HOST));
    }

    #[test]
    fn invitation_links_fit_a_terminal_code() {
        let link = format!("https://192.168.100.200:27659/#pair={}", "a".repeat(180));
        assert!(qr_code(&link).unwrap().lines().count() > 10);
    }
}
