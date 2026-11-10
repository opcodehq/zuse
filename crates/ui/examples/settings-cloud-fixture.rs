//! Settings review fixture: the real Shell over a scripted engine with
//! synthetic Cloud, billing and usage data. Nothing here is a live account.
//! Run under Xvfb with `ZUSE_OPEN_ROUTE=settings/<section>`, then capture.
use chrono::{Duration, Utc};
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, point, px, size};
use serde_json::{Value, json};
use std::sync::Arc;
use zuse_protocol::methods;
use zuse_protocol::usage::{
    CostStatus, Group, Overview, SessionsPage, SourceStatus, Summary, TokenCounts,
};
use zuse_rpc::{RpcError, RpcReply, RpcService};
use zuse_ui::*;

const OWNER: &str = "fixture-owner";
const DAY: i64 = 86_400_000;

fn now() -> i64 {
    Utc::now().timestamp_millis()
}
fn bound(data: Value) -> Result<RpcReply, RpcError> {
    RpcReply::value(&json!({"accountId": OWNER, "organizationId": null, "data": data}))
}
fn iso(days_ago: i64) -> String {
    (Utc::now() - Duration::days(days_ago)).to_rfc3339()
}

fn repositories() -> Value {
    let repos = [
        ("swarajbachu/zuse", 1, true),
        ("swarajbachu/spacekey", 63, false),
        ("swarajbachu/framezero", 21, false),
        ("swarajbachu/userbubble", 30, true),
        ("swarajbachu/www", 49, false),
        ("opcodehq/zuse", 0, true),
        ("opcodehq/zkit", 2, true),
        ("opcodehq/web", 8, true),
        ("opcodehq/computer-use", 9, false),
        ("opcodehq/postkit", 35, true),
    ];
    json!(
        repos
            .iter()
            .map(|(name, ago, private)| json!({
                "nameWithOwner": name,
                "description": null,
                "sshUrl": format!("git@github.com:{name}.git"),
                "httpsUrl": format!("https://github.com/{name}"),
                "isPrivate": private,
                "defaultBranch": "main",
                "updatedAt": iso(*ago),
            }))
            .collect::<Vec<_>>()
    )
}

fn projects() -> Value {
    let rows = [
        ("p-zuse", "opcodehq/zuse", "private"),
        ("p-web", "opcodehq/web", "private"),
        ("p-space", "swarajbachu/spacekey", "public"),
    ];
    json!({"projects": rows.iter().map(|(id, name, vis)| json!({
        "projectId": id, "repositoryIdentity": format!("github.com/{name}"),
        "repositoryUrl": format!("https://github.com/{name}"), "displayName": name,
        "defaultBranch": "main", "visibility": vis, "state": "ready",
        "activeBuilds": {}, "latestBuilds": {}, "createdAt": now() - 9 * DAY, "updatedAt": now() - DAY,
    })).collect::<Vec<_>>()})
}

/// Computers that ran this period, named by their chats.
const MACHINES: [(&str, &str, &str); 5] = [
    ("ws-login", "chat-login", "zuse/fix-login-redirect"),
    ("ws-billing", "chat-billing", "zuse/usage-breakdown"),
    ("ws-guide", "chat-guide", "zuse/cloud-setup-guide"),
    ("ws-docs", "chat-docs", "web/pricing-copy"),
    ("ws-flaky", "chat-flaky", "zuse/flaky-sync-test"),
];
const TITLES: [&str; 5] = [
    "Fix the login redirect loop",
    "Break usage down by computer",
    "Guided Cloud setup",
    "Rewrite the pricing page copy",
    "Find the flaky sync test",
];

fn usage_items(start: i64) -> Value {
    let mut items = vec![];
    let mut n = 0;
    for day in 0..6i64 {
        for (i, (ws, _, _)) in MACHINES.iter().enumerate() {
            if (day + i as i64) % 3 == 2 {
                continue;
            }
            let minutes = 12 + ((day * 7 + i as i64 * 11) % 40);
            let begin = start + day * DAY + (i as i64 + 9) * 3_600_000;
            n += 1;
            items.push(json!({"entryId": format!("e{n:04}"), "resourceKind": "workspace", "resourceId": ws,
                "provider": "boxd", "startedAt": begin, "endedAt": begin + minutes * 60_000,
                "vcpuCount": if i == 1 { 4 } else { 2 }, "memoryMib": if i == 1 { 8192 } else { 4096 },
                "providerCostMicros": minutes * if i == 1 { 9_000 } else { 4_500 }, "status": "confirmed"}));
        }
    }
    for (k, day) in [0i64, 3].iter().enumerate() {
        let begin = start + day * DAY + 8 * 3_600_000;
        items.push(json!({"entryId": format!("b{k}"), "resourceKind": "build", "resourceId": format!("build-{k}"),
            "provider": "boxd", "startedAt": begin, "endedAt": begin + 9 * 60_000, "vcpuCount": 4, "memoryMib": 8192,
            "providerCostMicros": 81_000, "status": "confirmed"}));
    }
    items.push(json!({"entryId": "s0", "resourceKind": "other", "usageKind": "snapshot-storage", "resourceId": "image",
        "provider": "boxd", "startedAt": start, "endedAt": now(), "vcpuCount": 0, "memoryMib": 0,
        "providerCostMicros": 310_000, "status": "provisional"}));
    let begin = now() - 25 * 60_000;
    items.push(json!({"entryId": "live", "resourceKind": "workspace", "resourceId": "ws-guide", "provider": "boxd",
        "startedAt": begin, "endedAt": now(), "vcpuCount": 2, "memoryMib": 4096, "providerCostMicros": 112_500, "status": "provisional"}));
    json!({"items": items})
}

fn summary(start: i64) -> Value {
    json!({"currency": "USD", "status": "active", "periodStart": start, "periodEnd": start + 30 * DAY,
        "basePriceMicros": 40_000_000, "includedProviderCostMicros": 35_000_000, "providerCostMicros": 8_420_000,
        "storageCostMicros": 310_000, "includedUsedMicros": 8_420_000, "includedRemainingMicros": 26_580_000,
        "overageProviderCostMicros": 0, "overageChargeMicros": 0, "overageCapMicros": 20_000_000,
        "markupBasisPoints": 500, "currentInvoiceEstimateMicros": 40_000_000, "usageProvisional": false})
}

fn tokens(scale: u64) -> Summary {
    Summary {
        tokens: TokenCounts {
            input_tokens: 41_000 * scale,
            output_tokens: 6_200 * scale,
            cache_read_tokens: 310_000 * scale,
            cache_creation_tokens: 9_000 * scale,
            reasoning_tokens: 2_100 * scale,
        },
        cost_usd: Some(0.42 * scale as f64),
        cost_status: CostStatus::Known,
        record_count: 12 * scale as usize,
        possible_duplicate_count: 0,
    }
}
fn group(key: &str, label: &str, days_ago: i64, scale: u64, source: &str) -> Group {
    let at = Utc::now() - Duration::days(days_ago);
    Group {
        key: key.into(),
        label: label.into(),
        started_at: Some(at),
        ended_at: Some(at + Duration::hours(1)),
        source_ids: vec![source.into()],
        summary: tokens(scale),
    }
}
fn overview() -> Overview {
    let groups = (0..30)
        .rev()
        .map(|d| group(&format!("d{d}"), "", d, 1 + ((d * 7) % 9) as u64, "codex"))
        .collect();
    Overview {
        bucket: "day".into(),
        generated_at: Utc::now(),
        summary: tokens(140),
        session_count: 38,
        previous_summary: Some(tokens(96)),
        previous_session_count: Some(27),
        groups,
        by_source: vec![
            group("codex", "Codex", 0, 92, "codex"),
            group("claude", "Claude Code", 0, 48, "claude"),
        ],
        by_model: vec![
            group("gpt", "GPT-5.5", 0, 81, "codex"),
            group("sonnet", "Claude Sonnet 5.5", 0, 48, "claude"),
            group("mini", "GPT-5.5 mini", 0, 11, "codex"),
        ],
        by_project: vec![
            group("zuse", "zuse", 0, 104, "codex"),
            group("web", "web", 0, 36, "claude"),
        ],
        previous_by_source: vec![],
        previous_by_model: vec![],
        previous_by_project: vec![],
        sources: vec![
            SourceStatus {
                id: "codex".into(),
                label: "Codex".into(),
                detected: true,
                record_count: 1104,
                paths: vec![],
                warning: None,
            },
            SourceStatus {
                id: "claude".into(),
                label: "Claude Code".into(),
                detected: true,
                record_count: 576,
                paths: vec![],
                warning: None,
            },
        ],
    }
}
fn sessions() -> SessionsPage {
    let rows: Vec<Group> = TITLES
        .iter()
        .enumerate()
        .map(|(i, t)| {
            group(
                &format!("s{i}"),
                t,
                i as i64,
                30 - 5 * i as u64,
                if i % 2 == 0 { "codex" } else { "claude" },
            )
        })
        .collect();
    SessionsPage {
        total: rows.len(),
        rows,
        next_offset: None,
    }
}

struct Controller {
    start: i64,
}
#[async_trait::async_trait]
impl RpcService for Controller {
    async fn handle(&self, method: &str, _params: Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::LOCAL_DEVICE => RpcReply::value(
                &json!({"deviceId":"fixture-local","workspaceScope":"local","capabilities":[]}),
            ),
            methods::CLOUD_PROJECTS => bound(projects()),
            methods::CLOUD_GITHUB => bound(json!({
                "configured": true, "canManageInstallations": true,
                "user": {"login": "swarajbachu", "name": "Swaraj", "email": "you@example.com"},
                "installations": [
                    {"installationId": 101, "accountLogin": "swarajbachu", "accountType": "User", "repositorySelection": "all", "suspended": false},
                    {"installationId": 202, "accountLogin": "opcodehq", "accountType": "Organization", "repositorySelection": "selected", "suspended": false}
                ],
                "repositories": repositories(),
            })),
            methods::CLOUD_AGENT_CONNECTIONS => bound(json!({"connections": [
                {"connectionId": "codex", "agent": "codex", "method": "device-code", "state": "connected", "accountLabel": "you@example.com", "expiresAt": null, "version": 1, "errorCode": null},
                {"connectionId": "claude", "agent": "claude", "method": "setup-token", "state": "connected", "accountLabel": "Claude Max", "expiresAt": null, "version": 1, "errorCode": null}
            ], "supportsDeviceReconnect": true})),
            methods::CLOUD_MACHINE => bound(
                json!({"revision": 3, "createdAt": now() - 2 * DAY, "imageState": "ready", "imageError": null,
                "recipe": {"version": 1, "setupScript": "curl -fsSL https://bun.sh/install | bash\nnpm i -g pnpm@9 turbo\napt-get install -y ripgrep jq",
                    "skills": [{"name": "release-notes", "content": "Write release notes from merged PRs."}],
                    "instructions": "Prefer small commits. Run the tests that cover what you changed before pushing.",
                    "files": [], "setupTimeoutSeconds": 900}}),
            ),
            methods::CLOUD_PROJECT_ENVIRONMENT => bound(
                json!({"projectId": _params["projectId"], "revision": 2, "source": "project", "createdAt": now() - DAY,
                "recipe": {"version": 1, "scripts": {"initialize": "pnpm install", "refresh": "pnpm install --frozen-lockfile"},
                    "startup": [{"name": "web", "command": "pnpm dev --port 3000", "port": 3000, "timeoutSeconds": 60}],
                    "environment": {"NODE_ENV": "development", "API_URL": "http://localhost:3000"}, "secretBindings": ["sec-1"], "setupTimeoutSeconds": 900}}),
            ),
            methods::CLOUD_PROJECT_SECRETS => bound(json!({"secrets": [
                {"secretId": "sec-1", "name": "DATABASE_URL", "revision": 1, "updatedAt": now() - 3 * DAY},
                {"secretId": "sec-2", "name": "STRIPE_TEST_KEY", "revision": 2, "updatedAt": now() - DAY}
            ]})),
            methods::CLOUD_BILLING_SUMMARY => bound(summary(self.start)),
            methods::CLOUD_BILLING_USAGE => bound(usage_items(self.start)),
            methods::CLOUD_BILLING_ENTITLEMENTS => bound(
                json!({"entitlements": [{"offerId": "cloud-workspace-standard-v1", "status": "active", "paidThrough": self.start + 30 * DAY}]}),
            ),
            methods::CLOUD_PREPAID_BALANCE => bound(
                json!({"available": true, "creditCents": 1250, "debitCents": 0, "currency": "USD"}),
            ),
            methods::CLOUD_WORKSPACES => {
                RpcReply::value(&json!({"accountId": OWNER, "organizationId": null,
                "workspaces": MACHINES.iter().map(|(ws, chat, branch)| json!({"workspaceId": ws, "chatId": chat, "branch": branch, "state": "ready"})).collect::<Vec<_>>()}))
            }
            zuse_protocol::usage::OVERVIEW => RpcReply::value(&overview()),
            zuse_protocol::usage::SESSIONS => RpcReply::value(&sessions()),
            methods::LIST_AGENT_ACCOUNTS => {
                RpcReply::value(&zuse_protocol::AgentAccountsSnapshot::default())
            }
            _ => Err(RpcError::Capability(format!("fixture.{method}"))),
        }
    }
}

fn main() {
    tracing_subscriber::fmt().with_env_filter("error").init();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let _enter = runtime.enter();
    let start = now() - 5 * DAY - 3 * 3_600_000;
    let client = zuse_rpc::memory_client(Arc::new(Controller { start }));
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().to_path_buf();
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init(cx);
        gpui_base::init(cx);
        let settings = settings::UiSettings::default();
        settings::init(settings.clone(), data.clone(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(settings.ui_font_family.clone(), settings.ui_font_size,
            settings.terminal_font_family.clone(), settings.terminal_font_size,
            settings.code_font_family.clone(), settings.code_font_size, fonts, cx);
        theme_library::init(data.clone(), cx);
        appearance::init(appearance::AppearanceMode::Dark, settings.theme_selection,
            settings.accent, settings.surface, cx);
        history::init(settings.git_history_columns, settings.git_history_column_widths,
            settings.git_history_column_order, settings.git_history_author_display, cx);
        composer::init(cx, settings.composer_send_behavior);
        terminal::panel::init(cx);
        app_menus::init(cx);
        let state = cx.new(|_| {
            let mut s = state::AppState::new();
            s.connection = zuse_protocol::view::ConnectionStatus::Ready;
            s.workspace_scope = Some(zuse_protocol::WorkspaceScope::Local);
            s.local_device_id = Some("fixture-local".into());
            s.devices = vec![serde_json::from_value(json!({"id":"fixture-local","name":"Studio Mac","platform":"macos","lastSeenAt":null})).unwrap()];
            s.auto_selected = true;
            s.spaces_synced = true;
            s.chats_synced = true;
            s.apply_cloud_account(json!({"user":{"id": OWNER,"email":"you@example.com"},"orgId":null}));
            s.chats = MACHINES.iter().zip(TITLES).map(|((_, chat, branch), title)| serde_json::from_value(json!({
                "id": chat, "deviceId": "fixture-local", "title": title, "branch": branch, "archived": false,
                "createdAt": "2026-11-02T12:00:00Z"})).unwrap()).collect();
            s.fixture_cloud_sidebar_catalog(client, &json!({}), &json!({"accountId": OWNER, "organizationId": null, "workspaces": []}));
            s
        });
        let boot = EngineBootConfig { target: None, data_dir: data, ipc_port: 0, edge_url: String::new(),
            edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::Codex };
        let width: f32 = std::env::var("FIXTURE_WIDTH").ok().and_then(|v| v.parse().ok()).unwrap_or(1280.);
        let height: f32 = std::env::var("FIXTURE_HEIGHT").ok().and_then(|v| v.parse().ok()).unwrap_or(1500.);
        let window = cx.open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(0.), px(0.)), size(px(width), px(height))))),
            ..Default::default()
        }, |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx))).unwrap();
        state.update(cx, |_, cx| cx.notify());
        cx.activate(true);
        let out = std::env::var("FIXTURE_OUT").ok();
        let compute = std::env::var("FIXTURE_COMPUTE").is_ok();
        cx.spawn(async move |cx| {
            let delay = std::env::var("FIXTURE_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(2500);
            cx.background_executor().timer(std::time::Duration::from_millis(delay)).await;
            if compute {
                window.update(cx, |shell, _, cx| shell.fixture_usage_compute(cx)).unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(2000)).await;
            }
            let Some(out) = out else { return };
            let any: gpui::AnyWindowHandle = window.into();
            any.update(cx, |_, window, cx| {
                window.draw(cx).clear();
                window.present_if_needed();
                match window.render_to_image() {
                    Ok(image) => image.save(&out).expect("save"),
                    Err(_) => {
                        let status = std::process::Command::new("import")
                            .args(["-window", "root", &out])
                            .status()
                            .expect("import");
                        assert!(status.success(), "X11 capture failed");
                    }
                }
            }).unwrap();
            std::process::exit(0);
        }).detach();
    });
}
