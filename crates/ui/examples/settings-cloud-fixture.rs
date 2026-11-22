//! Settings review fixture: the real Shell over a scripted engine with
//! synthetic Cloud, billing and usage data. Nothing here is a live account.
//! Run under Xvfb with `ZUSE_OPEN_ROUTE=settings/<section>`, then capture.
//! `FIXTURE_PLAN` picks the plan state (cloud, trial, overage, max, hold,
//! trial-hold, none) and `FIXTURE_THEME=light` the light theme.
//! `FIXTURE_FRESH=1` answers
//! as a just-subscribed account: no GitHub, repositories or agent accounts,
//! and the standard machine. `FIXTURE_ONBOARDING=<step>` opens first-run
//! onboarding at `welcome`, `repositories`, `setup`, `agents` or `start`.
//! `FIXTURE_NO_REPOS=1` keeps GitHub connected but adds no repositories.
//! `FIXTURE_IMAGE` sets the account image (`queued`, `building`, `ready`,
//! `failed`, or `none` for no machine version yet; default `ready`).
//! `FIXTURE_LOCAL_SPACE=1` adds a local checkout of a Cloud repository
//! (`2` adds a second checkout of it on this computer).
//! `FIXTURE_ACCOUNT=signed-out` starts signed out. `FIXTURE_ARCHIVED=1`
//! archives the first chat. `FIXTURE_CLICKS="x,y;x,y"` clicks those window
//! points (via XTest, under Xvfb) before the capture, and
//! `FIXTURE_TYPE=text` then types into the focused field, and
//! `FIXTURE_KEYS=Down,space` then presses those X keysyms.
//! `FIXTURE_NEW_INSTALL=1` adds the organization's repositories on the
//! second GitHub read, as a fresh installation would.
//! `FIXTURE_AGENTS=multi` adds a second Codex account. Device sign-in hands
//! out a code and keeps waiting; `FIXTURE_LOGIN=connect` finishes it after a
//! few status checks.
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

/// About twenty repositories across a user and an organization. Avatar URLs
/// don't resolve here, so rows show their initials tiles.
const REPOSITORIES: [(&str, i64, bool); 21] = [
    ("swarajbachu/zuse", 1, true),
    ("swarajbachu/framezero", 21, false),
    ("swarajbachu/userbubble", 30, true),
    ("swarajbachu/swarajbachu", 42, false),
    ("swarajbachu/lint", 49, false),
    ("swarajbachu/www", 50, false),
    ("swarajbachu/spacekey", 63, false),
    ("swarajbachu/omake", 77, false),
    ("swarajbachu/botdirectory.ai", 80, false),
    ("opcodehq/zuse", 0, true),
    ("opcodehq/zkit", 2, true),
    ("opcodehq/computer-use", 7, false),
    ("opcodehq/brokeboy", 8, true),
    ("opcodehq/web", 9, true),
    ("opcodehq/delulu", 10, false),
    ("opcodehq/postkit", 35, true),
    ("opcodehq/agent-stuff", 49, true),
    ("opcodehq/morningpage", 105, false),
    ("opcodehq/design-tokens", 140, false),
    ("opcodehq/edge", 190, true),
    ("opcodehq/runbooks", 400, true),
];

fn repositories(owners: &[&str]) -> Value {
    json!(
        REPOSITORIES
            .iter()
            .filter(|(name, _, _)| owners.iter().any(|o| name.starts_with(&format!("{o}/"))))
            .map(|(name, ago, private)| json!({
                "nameWithOwner": name,
                "description": null,
                "sshUrl": format!("git@github.com:{name}.git"),
                "httpsUrl": format!("https://github.com/{name}"),
                "isPrivate": private,
                "defaultBranch": "main",
                "updatedAt": iso(*ago),
                "ownerAvatarUrl": format!("https://avatars.invalid/{}", name.split('/').next().unwrap()),
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

/// Which plan state the fixture shows (`FIXTURE_PLAN`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum PlanState {
    /// Cloud, a third of the way through its machine minutes.
    Cloud,
    /// The $1 trial, part used.
    Trial,
    /// Cloud past its allowance, paying as it goes under the cap.
    Overage,
    /// Cloud Max with a large computer.
    Max,
    /// Cloud with the pay-as-you-go cap reached: computers paused.
    Hold,
    /// The trial used up: computers paused.
    TrialHold,
    /// Not subscribed: plans and the $1 trial.
    None,
}
impl PlanState {
    fn from_env() -> Self {
        match std::env::var("FIXTURE_PLAN").as_deref() {
            Ok("trial") => Self::Trial,
            Ok("overage") => Self::Overage,
            Ok("max") => Self::Max,
            Ok("hold") => Self::Hold,
            Ok("trial-hold") => Self::TrialHold,
            Ok("none") => Self::None,
            _ => Self::Cloud,
        }
    }
    fn trial(self) -> bool {
        matches!(self, Self::Trial | Self::TrialHold)
    }
}

fn usage_items(start: i64, plan: PlanState) -> Value {
    let mut items = vec![];
    let mut n = 0;
    // Busier days push the overage states past the allowance.
    let scale = match plan {
        PlanState::Overage | PlanState::Hold => 16,
        PlanState::Max => 6,
        PlanState::Trial | PlanState::TrialHold => 3,
        _ => 4,
    };
    let days = if plan.trial() { 4 } else { 6 };
    for day in 0..days {
        for (i, (ws, _, _)) in MACHINES.iter().enumerate() {
            if (day + i as i64) % 3 == 2 {
                continue;
            }
            let awake = (12 + ((day * 7 + i as i64 * 11) % 40)) * scale;
            let large = plan == PlanState::Max && i == 1;
            let rate = if large { 8 } else { 4 };
            let begin = start + day * DAY + (i as i64 + 9) * 3_600_000;
            n += 1;
            items.push(json!({"entryId": format!("e{n:04}"), "resourceKind": "workspace", "resourceId": ws,
                "provider": "cloud", "startedAt": begin, "endedAt": begin + awake * 60_000,
                "vcpuCount": if large { 8 } else { 4 }, "memoryMib": if large { 32_768 } else { 16_384 },
                "providerCostMicros": awake * rate * 595, "status": "confirmed",
                "awakeMs": awake * 60_000, "machineRate": rate, "machineMinutes": awake * rate}));
        }
    }
    for (k, day) in [0i64, 3].iter().enumerate() {
        let begin = start + day * DAY + 8 * 3_600_000;
        items.push(json!({"entryId": format!("b{k}"), "resourceKind": "build", "resourceId": format!("build-{k}"),
            "provider": "cloud", "startedAt": begin, "endedAt": begin + 9 * 60_000, "vcpuCount": 4, "memoryMib": 16_384,
            "providerCostMicros": 21_420, "status": "confirmed", "awakeMs": 9 * 60_000, "machineRate": 4, "machineMinutes": 36}));
    }
    items.push(json!({"entryId": "s0", "resourceKind": "other", "usageKind": "snapshot-storage", "resourceId": "image",
        "provider": "cloud", "startedAt": start, "endedAt": now(), "vcpuCount": 0, "memoryMib": 0,
        "providerCostMicros": 310_000, "status": "provisional", "machineMinutes": 0}));
    let begin = now() - 25 * 60_000;
    items.push(json!({"entryId": "live", "resourceKind": "workspace", "resourceId": "ws-guide", "provider": "cloud",
        "startedAt": begin, "endedAt": now(), "vcpuCount": 4, "memoryMib": 16_384, "providerCostMicros": 59_500,
        "status": "provisional", "awakeMs": 25 * 60_000, "machineRate": 4, "machineMinutes": 100}));
    json!({"items": items})
}

/// Machine minutes the synthetic usage adds up to, so the plan agrees with it.
fn used_minutes(start: i64, plan: PlanState) -> i64 {
    usage_items(start, plan)["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|item| item["machineMinutes"].as_i64())
        .sum()
}

fn summary(start: i64, plan: PlanState) -> Value {
    let end = start + if plan.trial() { 7 * DAY } else { 30 * DAY };
    let used = used_minutes(start, plan);
    let (id, name, price, included, max) = match plan {
        PlanState::Trial | PlanState::TrialHold => {
            ("cloud-trial", "Cloud trial", 1_000_000, 8_400, false)
        }
        PlanState::Max => ("cloud-max", "Cloud Max", 50_000_000, 84_000, true),
        _ => ("cloud", "Cloud", 20_000_000, 33_600, false),
    };
    let cap = 25_000_000i64;
    let used = match plan {
        PlanState::TrialHold => used.max(included),
        // The cap reached: $25 of extra minutes past the allowance.
        PlanState::Hold => included + 42_000,
        _ => used,
    };
    let extra = (used - included).max(0);
    let charge = zuse_protocol::cloud_billing::plans::machine_minutes_micros(extra);
    let (hold, reason) = match plan {
        PlanState::Hold => (true, Some("cap-reached")),
        PlanState::TrialHold => (true, Some("trial-allowance-used")),
        _ => (false, None),
    };
    let mut plan_usage = json!({"planId": id, "planName": name, "priceMicros": price, "trial": plan.trial(),
        "renewsAt": end, "includedMachineMinutes": included, "usedMachineMinutes": used.min(included) + extra,
        "remainingMachineMinutes": (included - used).max(0), "extraMachineMinutes": if plan.trial() { 0 } else { extra },
        "extraChargeMicros": if plan.trial() { 0 } else { charge }, "overageCapMicros": cap, "payAsYouGo": !plan.trial(),
        "usageHold": hold, "defaultMachineRate": 4, "maxVcpus": if max { 8 } else { 4 },
        "maxMemoryMib": if max { 32_768 } else { 16_384 }});
    if let Some(reason) = reason {
        plan_usage["holdReason"] = json!(reason);
    }
    if plan.trial() {
        plan_usage["trialEndsAt"] = json!(end);
    }
    json!({"currency": "USD", "status": "active", "periodStart": start, "periodEnd": end,
        "basePriceMicros": price, "includedProviderCostMicros": price, "providerCostMicros": 8_420_000,
        "storageCostMicros": 310_000, "includedUsedMicros": 8_420_000, "includedRemainingMicros": 11_580_000,
        "overageProviderCostMicros": 0, "overageChargeMicros": charge, "overageCapMicros": cap,
        "markupBasisPoints": 0, "currentInvoiceEstimateMicros": price + charge, "usageProvisional": false,
        "plan": plan_usage})
}

fn entitlements(start: i64, plan: PlanState) -> Value {
    let catalog = zuse_protocol::cloud_billing::plans::catalog(
        plan == PlanState::None,
        &|_| true,
        zuse_protocol::cloud_billing::plans::ENTERPRISE_CONTACT_FALLBACK,
    );
    let offer = match plan {
        PlanState::Max => "cloud-pro-v1",
        _ => "cloud-starter-v1",
    };
    let entitlements = if plan == PlanState::None {
        json!([])
    } else {
        json!([{"offerId": offer, "status": "active", "paidThrough": start + 30 * DAY}])
    };
    json!({"entitlements": entitlements, "catalog": catalog})
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

/// The account's machine and image as `FIXTURE_IMAGE` describes it.
fn machine(image: &str) -> Value {
    let recipe = json!({"version": 1, "setupScript": "curl -fsSL https://bun.sh/install | bash\nnpm i -g pnpm@9 turbo\napt-get install -y ripgrep jq",
        "skills": [{"name": "release-notes", "content": "Write release notes from merged PRs."}],
        "instructions": "Prefer small commits. Run the tests that cover what you changed before pushing.",
        "files": [], "setupTimeoutSeconds": 900});
    let baked = json!([
        {"repositoryIdentity": "github.com/opcodehq/zuse", "state": "baked", "commit": "a".repeat(40)},
        {"repositoryIdentity": "github.com/opcodehq/web", "state": "baked", "commit": "b".repeat(40)},
        {"repositoryIdentity": "github.com/swarajbachu/spacekey", "state": "baked", "commit": "c".repeat(40)}
    ]);
    match image {
        "none" => json!({"revision": 0, "createdAt": 0, "imageState": "none", "imageError": null,
            "recipe": {"version": 1, "setupScript": "", "skills": [], "instructions": "", "files": [], "setupTimeoutSeconds": 900}}),
        "queued" => {
            json!({"revision": 3, "createdAt": now() - DAY, "imageState": "queued", "imageError": null, "recipe": recipe,
            "image": {"state": "queued", "reason": "recipe", "recipeRevision": 3, "updatedAt": now()}})
        }
        "building" => {
            json!({"revision": 3, "createdAt": now() - DAY, "imageState": "building", "imageError": null, "recipe": recipe,
            "image": {"state": "building", "phase": "setup", "reason": "recipe", "recipeRevision": 3, "updatedAt": now(), "repositories": baked}})
        }
        "failed" => {
            json!({"revision": 3, "createdAt": now() - DAY, "imageState": "failed", "imageError": "user_image_setup_failed", "recipe": recipe,
            "image": {"state": "failed", "reason": "recipe", "recipeRevision": 3, "error": "user_image_setup_failed", "updatedAt": now(),
                "logExcerpt": "+ npm i -g pnpm@9 turbo\nnpm ERR! code E404\nnpm ERR! 404 Not Found - GET https://registry.npmjs.org/turbo"}})
        }
        _ => {
            json!({"revision": 3, "createdAt": now() - 2 * DAY, "imageState": "ready", "imageError": null, "recipe": recipe,
            "image": {"state": "ready", "reason": "recipe", "recipeRevision": 3, "builtAt": now() - DAY, "updatedAt": now() - DAY, "repositories": baked}})
        }
    }
}

/// What the composer's pickers read to offer Cloud: the repositories, a
/// ready computer and one connected agent.
fn cloud_context() -> Value {
    let projects: Vec<Value> = projects()["projects"].as_array().unwrap().iter().map(|p| json!({
        "projectId": p["projectId"], "repositoryIdentity": p["repositoryIdentity"], "displayName": p["displayName"],
        "repositoryUrl": p["repositoryUrl"], "defaultBranch": "main", "state": "ready", "canCreate": true})).collect();
    json!({"accountId": OWNER, "organizationId": null, "ownerId": OWNER, "apiOrigin": "https://cloud.example.test",
        "projects": projects, "commandAdmissionVersion": 2,
        "computerCapabilities": {"defaultProvider": "boxd", "providers": [{"id": "boxd", "configured": true, "managedBaseReady": true,
            "commandAdmissionVersion": 2, "preservesDisk": true, "preservesMemory": true, "supportsSnapshots": true, "supportsDocker": false,
            "sizes": [{"id": "default", "label": "Standard", "vcpus": 4, "memoryMib": 16384, "diskGib": 40}], "unavailableReason": null}],
            "computerSizes": [{"id": "default", "label": "Standard · 4 vCPU · 16 GB", "vcpus": 4, "memoryMib": 16384, "machineRate": 4, "available": true}]},
        "providerAgentCatalogs": {"boxd": {"providers": [{"id": "codex", "name": "Codex", "defaultModel": "gpt-5.5",
            "models": [{"id": "gpt-5.5", "label": "GPT-5.5"}]}]}},
        "agentConnections": [{"agent": "codex", "state": "connected", "connectionId": "codex"}]})
}

struct Controller {
    start: i64,
    /// `FIXTURE_IMAGE`.
    image: String,
    plan: PlanState,
    /// A just-subscribed account with nothing set up yet.
    fresh: bool,
    /// GitHub connected, no repositories added yet.
    no_repos: bool,
    /// Device sign-in status checks so far.
    polls: std::sync::atomic::AtomicU32,
    /// An organization installation arrives after the first GitHub read.
    new_install: bool,
    github_reads: std::sync::atomic::AtomicU32,
}
impl Controller {
    fn login(&self, state: &str) -> Result<RpcReply, RpcError> {
        bound(
            json!({"operationId": "op-1", "connectionId": "codex-new", "agent": "codex",
            "state": state, "verificationUrl": "https://example.com/device",
            "verificationCode": "WDJB-MJHT", "expiresAt": now() + 15 * 60_000, "errorCode": null}),
        )
    }
    fn signed_in(&self) -> bool {
        std::env::var("FIXTURE_LOGIN").as_deref() == Ok("connect")
            && self.polls.load(std::sync::atomic::Ordering::SeqCst) >= 3
    }
}
#[async_trait::async_trait]
impl RpcService for Controller {
    async fn handle(&self, method: &str, _params: Value) -> Result<RpcReply, RpcError> {
        match method {
            methods::CLOUD_AGENT_LOGIN => return self.login("authorizing"),
            methods::CLOUD_AGENT_LOGIN_STATUS => {
                self.polls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                return self.login(if self.signed_in() {
                    "connected"
                } else {
                    "authorizing"
                });
            }
            methods::CLOUD_AGENT_LOGIN_CANCEL => return self.login("cancelled"),
            methods::CLOUD_AGENT_DISCONNECT => return bound(json!({})),
            // Connecting takes a while, so "Adding…" rows can be captured.
            methods::CLOUD_PROJECT_CONNECT => {
                tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                return Err(RpcError::Failed("Fixture connect timed out.".into()));
            }
            methods::CLOUD_AGENT_CONNECTIONS if self.fresh && self.signed_in() => {
                return bound(json!({"connections": [
                    {"connectionId": "codex-new", "agent": "codex", "method": "device-code", "state": "connected", "accountLabel": "you@example.com", "expiresAt": null, "version": 1, "errorCode": null}
                ], "supportsDeviceReconnect": true}));
            }
            methods::CLOUD_AGENT_CONNECTIONS
                if !self.fresh && std::env::var("FIXTURE_AGENTS").as_deref() == Ok("multi") =>
            {
                return bound(json!({"connections": [
                    {"connectionId": "codex", "agent": "codex", "method": "device-code", "state": "connected", "accountLabel": "you@example.com", "expiresAt": null, "version": 1, "errorCode": null},
                    {"connectionId": "codex-work", "agent": "codex", "method": "device-code", "state": "connected", "accountLabel": "work@example.com", "expiresAt": null, "version": 1, "errorCode": null},
                    {"connectionId": "claude", "agent": "claude", "method": "setup-token", "state": "connected", "accountLabel": "Claude Max", "expiresAt": null, "version": 1, "errorCode": null}
                ], "supportsDeviceReconnect": true, "supportsCredentialReconfiguration": true}));
            }
            _ => {}
        }
        if self.no_repos && method == methods::CLOUD_PROJECTS {
            return bound(json!({"projects": []}));
        }
        if self.fresh {
            match method {
                // Just subscribed: nothing used yet.
                methods::CLOUD_BILLING_SUMMARY => {
                    let mut summary = summary(self.start, self.plan);
                    let plan = &mut summary["plan"];
                    plan["remainingMachineMinutes"] = plan["includedMachineMinutes"].clone();
                    plan["usedMachineMinutes"] = json!(0);
                    plan["extraMachineMinutes"] = json!(0);
                    return bound(summary);
                }
                methods::CLOUD_BILLING_USAGE => return bound(json!({"items": []})),
                methods::CLOUD_PROJECTS => return bound(json!({"projects": []})),
                methods::CLOUD_GITHUB => {
                    return bound(json!({"configured": true, "canManageInstallations": true,
                        "user": null, "installations": [], "repositories": []}));
                }
                methods::CLOUD_AGENT_CONNECTIONS => {
                    return bound(json!({"connections": [], "supportsDeviceReconnect": true}));
                }
                methods::CLOUD_MACHINE => {
                    return bound(
                        json!({"revision": 0, "createdAt": now(), "imageState": "none",
                        "imageError": null, "recipe": {"version": 1, "setupScript": "", "skills": [],
                        "instructions": "", "files": [], "setupTimeoutSeconds": 900}}),
                    );
                }
                // Nothing to carry yet: saving queues no image.
                methods::CLOUD_MACHINE_SAVE => {
                    let mut saved = machine("none");
                    saved["revision"] = json!(1);
                    saved["recipe"] = _params["recipe"].clone();
                    return bound(saved);
                }
                _ => {}
            }
        }
        match method {
            methods::LOCAL_DEVICE => RpcReply::value(
                &json!({"deviceId":"fixture-local","workspaceScope":"local","capabilities":[]}),
            ),
            methods::CLOUD_PROJECTS => bound(projects()),
            methods::CLOUD_GITHUB => {
                // `FIXTURE_NEW_INSTALL=1`: the organization's installation
                // lands on the second read, as after installing it.
                let reads = self
                    .github_reads
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let owners: &[&str] = if self.new_install && reads == 0 {
                    &["swarajbachu"]
                } else {
                    &["swarajbachu", "opcodehq"]
                };
                let installations = [
                    json!({"installationId": 101, "accountLogin": "swarajbachu", "accountType": "User", "repositorySelection": "all", "suspended": false, "avatarUrl": "https://avatars.invalid/swarajbachu"}),
                    json!({"installationId": 202, "accountLogin": "opcodehq", "accountType": "Organization", "repositorySelection": "selected", "suspended": false, "avatarUrl": "https://avatars.invalid/opcodehq"}),
                ];
                bound(json!({
                    "configured": true, "canManageInstallations": true,
                    "user": {"login": "swarajbachu", "name": "Swaraj", "email": "you@example.com"},
                    "installations": installations[..owners.len()],
                    "repositories": repositories(owners),
                }))
            }
            methods::CLOUD_AGENT_CONNECTIONS => bound(json!({"connections": [
                {"connectionId": "codex", "agent": "codex", "method": "device-code", "state": "connected", "accountLabel": "you@example.com", "expiresAt": null, "version": 1, "errorCode": null},
                {"connectionId": "claude", "agent": "claude", "method": "setup-token", "state": "connected", "accountLabel": "Claude Max", "expiresAt": null, "version": 1, "errorCode": null}
            ], "supportsDeviceReconnect": true})),
            methods::CLOUD_MACHINE => bound(machine(&self.image)),
            // Saving or a rebuild starts the image build.
            methods::CLOUD_MACHINE_SAVE | methods::CLOUD_MACHINE_REBUILD => {
                bound(machine("building"))
            }
            methods::CLOUD_CONTEXT => RpcReply::value(&cloud_context()),
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
            methods::CLOUD_BILLING_SUMMARY | methods::CLOUD_BILLING_USAGE
                if self.plan == PlanState::None =>
            {
                Err(RpcError::Failed(
                    "An active cloud billing period is required.".into(),
                ))
            }
            methods::CLOUD_BILLING_SUMMARY => bound(summary(self.start, self.plan)),
            methods::CLOUD_BILLING_USAGE => bound(usage_items(self.start, self.plan)),
            methods::CLOUD_BILLING_ENTITLEMENTS => bound(entitlements(self.start, self.plan)),
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
    let plan = PlanState::from_env();
    let start = now() - if plan.trial() { 3 } else { 5 } * DAY - 3 * 3_600_000;
    let fresh = std::env::var("FIXTURE_FRESH").is_ok();
    let client = zuse_rpc::memory_client(Arc::new(Controller {
        start,
        plan,
        fresh,
        image: std::env::var("FIXTURE_IMAGE").unwrap_or_else(|_| "ready".into()),
        no_repos: std::env::var("FIXTURE_NO_REPOS").is_ok(),
        polls: Default::default(),
        new_install: std::env::var("FIXTURE_NEW_INSTALL").is_ok(),
        github_reads: Default::default(),
    }));
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
        let mode = if std::env::var("FIXTURE_THEME").as_deref() == Ok("light") {
            appearance::AppearanceMode::Light
        } else {
            appearance::AppearanceMode::Dark
        };
        appearance::init(mode, settings.theme_selection,
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
            if std::env::var("FIXTURE_ACCOUNT").as_deref() == Ok("signed-out") {
                s.apply_cloud_account(json!({"user": null, "orgId": null}));
            } else {
                s.apply_cloud_account(json!({"user":{"id": OWNER,"email":"you@example.com"},"orgId":null}));
                s.fixture_cloud_subscription(plan != PlanState::None);
            }
            s.chats = MACHINES.iter().zip(TITLES).map(|((_, chat, branch), title)| serde_json::from_value(json!({
                "id": chat, "deviceId": "fixture-local", "title": title, "branch": branch, "archived": false,
                "createdAt": "2026-11-02T12:00:00Z"})).unwrap()).collect();
            if std::env::var("FIXTURE_ARCHIVED").is_ok()
                && let Some(chat) = s.chats.first_mut()
            {
                chat.archived = true;
            }
            s.fixture_cloud_sidebar_catalog(client, &cloud_context(), &json!({"accountId": OWNER, "organizationId": null, "workspaces": []}));
            // `FIXTURE_LOCAL_SPACE=1`: a checkout of opcodehq/zuse on this
            // computer, the same repository as a Cloud one.
            if std::env::var("FIXTURE_LOCAL_SPACE").is_ok() {
                s.spaces.push(serde_json::from_value(json!({"id": "space-zuse", "deviceId": "fixture-local",
                    "path": "/Users/you/code/zuse", "name": "zuse", "gitDetected": true,
                    "remoteRepository": "github.com/opcodehq/zuse", "repositoryId": "github.com/opcodehq/zuse", "createdAt": "2026-11-01T12:00:00Z"})).unwrap());
            }
            // `FIXTURE_LOCAL_SPACE=2`: a second checkout of it on this computer.
            if std::env::var("FIXTURE_LOCAL_SPACE").as_deref() == Ok("2") {
                s.spaces.push(serde_json::from_value(json!({"id": "space-zuse-copy", "deviceId": "fixture-local",
                    "path": "/Users/you/conductor/zuse", "name": "zuse", "gitDetected": true,
                    "remoteRepository": "github.com/opcodehq/zuse", "repositoryId": "github.com/opcodehq/zuse", "createdAt": "2026-11-02T12:00:00Z"})).unwrap());
            }
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
        let onboarding = std::env::var("FIXTURE_ONBOARDING").ok();
        cx.spawn(async move |cx| {
            let delay = std::env::var("FIXTURE_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(2500);
            cx.background_executor().timer(std::time::Duration::from_millis(delay)).await;
            if let Some(step) = onboarding {
                window
                    .update(cx, |shell, window, cx| {
                        shell.open_cloud_onboarding(Some(step.as_str()), window, cx)
                    })
                    .unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
            }
            if compute {
                window.update(cx, |shell, _, cx| shell.fixture_usage_compute(cx)).unwrap();
                cx.background_executor().timer(std::time::Duration::from_millis(2000)).await;
            }
            for point in std::env::var("FIXTURE_CLICKS").unwrap_or_default().split(';').filter(|p| !p.is_empty()) {
                let status = std::process::Command::new("python3")
                    .args(["-c", XTEST, "click", point])
                    .status()
                    .expect("click");
                assert!(status.success(), "click failed");
                cx.background_executor().timer(std::time::Duration::from_millis(900)).await;
            }
            if let Ok(text) = std::env::var("FIXTURE_TYPE") {
                let status = std::process::Command::new("python3")
                    .args(["-c", XTEST, "type", &text])
                    .status()
                    .expect("type");
                assert!(status.success(), "typing failed");
                cx.background_executor().timer(std::time::Duration::from_millis(900)).await;
            }
            for key in std::env::var("FIXTURE_KEYS").unwrap_or_default().split(',').filter(|k| !k.is_empty()) {
                let status = std::process::Command::new("python3")
                    .args(["-c", XTEST, "key", key])
                    .status()
                    .expect("key");
                assert!(status.success(), "key press failed");
                cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
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

/// Clicks a point or types text into the focused field through XTest.
const XTEST: &str = r#"
import ctypes as c,sys,time
x=c.CDLL('libX11.so.6');t=c.CDLL('libXtst.so.6')
x.XOpenDisplay.argtypes=[c.c_char_p];x.XOpenDisplay.restype=c.c_void_p;d=x.XOpenDisplay(None);assert d
x.XFlush.argtypes=[c.c_void_p];t.XTestFakeMotionEvent.argtypes=[c.c_void_p,c.c_int,c.c_int,c.c_int,c.c_ulong];t.XTestFakeButtonEvent.argtypes=[c.c_void_p,c.c_uint,c.c_int,c.c_ulong]
x.XStringToKeysym.argtypes=[c.c_char_p];x.XStringToKeysym.restype=c.c_ulong
x.XKeysymToKeycode.argtypes=[c.c_void_p,c.c_ulong];x.XKeysymToKeycode.restype=c.c_uint
t.XTestFakeKeyEvent.argtypes=[c.c_void_p,c.c_uint,c.c_int,c.c_ulong]
if sys.argv[1]=='click':
    a,b=map(int,sys.argv[2].split(','))
    t.XTestFakeMotionEvent(d,-1,a,b,0);x.XFlush(d);time.sleep(.3)
    t.XTestFakeButtonEvent(d,1,1,0);t.XTestFakeButtonEvent(d,1,0,0);x.XFlush(d)
elif sys.argv[1]=='key':
    code=x.XKeysymToKeycode(d,x.XStringToKeysym(sys.argv[2].encode()));assert code
    t.XTestFakeKeyEvent(d,code,1,0);t.XTestFakeKeyEvent(d,code,0,0);x.XFlush(d)
else:
    def key(name,down):
        code=x.XKeysymToKeycode(d,x.XStringToKeysym(name.encode()));assert code
        t.XTestFakeKeyEvent(d,code,down,0)
    for ch in sys.argv[2]:
        up=ch.isupper()
        name={' ':'space','-':'minus','/':'slash','.':'period'}.get(ch,ch.lower())
        if up:key('Shift_L',1)
        key(name,1);key(name,0)
        if up:key('Shift_L',0)
    x.XFlush(d)
"#;
