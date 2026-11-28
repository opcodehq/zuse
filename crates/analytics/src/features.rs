//! Product actions only. Watches, polling, file contents and keystrokes are excluded.
use serde_json::Value;
use zuse_protocol::methods as m;

const METHODS: &[(&str, &str)] = &[
    (
        zuse_protocol::project_creation::START,
        "project creation started",
    ),
    (
        zuse_protocol::project_creation::CANCEL,
        "project creation cancelled",
    ),
    (
        zuse_protocol::project_creation::RETRY,
        "project creation retried",
    ),
    (
        zuse_protocol::external_chat::IMPORT,
        "conversation imported",
    ),
    (
        zuse_protocol::creation_source::RESOLVE,
        "creation source selected",
    ),
    (zuse_protocol::permission::UPDATE, "permission rule changed"),
    (zuse_protocol::permission::REMOVE, "permission rule removed"),
    (m::ADD_REPO, "repository added"),
    (m::CLONE_REPO, "repository cloned"),
    (m::CREATE_REPO, "repository created"),
    (m::CREATE_WORKTREE, "worktree created"),
    (m::DELETE_WORKTREE, "worktree deleted"),
    (m::SWITCH_REF, "reference switched"),
    (m::FETCH_ALL, "references fetched"),
    (m::GIT_WORKSPACE_ACTION, "git action"),
    (m::POST_CHANGE_REQUEST_COMMENT, "pull request commented"),
    (
        m::REPLY_CHANGE_REQUEST_THREAD,
        "pull request thread replied",
    ),
    (m::UPDATE_CHANGE_REQUEST, "pull request updated"),
    (m::UPDATE_CHANGE_REQUEST_AT_HEAD, "pull request updated"),
    (m::SET_PR_WATCH, "pull request watch changed"),
    (m::DISCARD_WORKING_TREE, "checkout discarded"),
    (m::OPEN_TERMINAL, "terminal opened"),
    (m::CLOSE_TERMINAL, "terminal closed"),
    (m::CREATE_THREAD_SESSION, "conversation tab created"),
    (m::FORK_SIDE_CHAT, "session forked"),
    (m::RETRY_DELIVERY, "delivery retried"),
    (m::QUEUE_MESSAGE, "message queued"),
    (m::UPDATE_QUEUED_MESSAGE, "queued message edited"),
    (m::MOVE_QUEUED_MESSAGE, "queued message reordered"),
    (m::REMOVE_QUEUED_MESSAGE, "queued message removed"),
    (m::SEND_QUEUED_MESSAGE_NOW, "queued message sent"),
    (m::STEER_QUEUED_MESSAGE_NOW, "queued message steered"),
    (m::MOVE_WORKSPACE_ENTRY, "workspace entry moved"),
    (m::DELETE_WORKSPACE_ENTRY, "workspace entry deleted"),
    (m::UPSERT_PROJECT_ACTION, "project action saved"),
    (m::DELETE_PROJECT_ACTION, "project action deleted"),
    (m::RUN_PROJECT_ACTION, "project action run"),
    (m::START_AGENT_LOGIN, "provider login"),
    (m::ACTIVATE_AGENT_ACCOUNT, "provider account selected"),
    (m::FORGET_AGENT_ACCOUNT, "provider account removed"),
    (m::INSTALL_HARNESS, "provider installed"),
    (m::SET_HARNESS_ENABLED, "provider enablement changed"),
    (m::SET_PROVIDER_MODELS, "provider models changed"),
    (m::MODEL_CONNECT, "model connection started"),
    (m::MODEL_DISCONNECT, "model connection removed"),
    (m::MODEL_PREFERRED, "preferred model connection changed"),
    (m::CREATE_PAIRING, "device pairing created"),
    (m::REVOKE_PAIRING, "device pairing revoked"),
    (m::START_BROWSER_ACCESS, "browser access started"),
    (m::STOP_BROWSER_ACCESS, "browser access stopped"),
    (m::START_DIRECT_ACCESS, "direct access started"),
    (m::STOP_DIRECT_ACCESS, "direct access stopped"),
    (m::START_RELAY_ACCESS, "relay access started"),
    (m::STOP_RELAY_ACCESS, "relay access stopped"),
    (m::OPEN_SIMULATOR, "simulator opened"),
    (m::CLOSE_SIMULATOR, "simulator closed"),
    (m::PREPARE_VOICE_V2, "voice preparation requested"),
    (m::STOP_VOICE_V2, "voice stopped"),
    (m::CLOUD_CREATE, "cloud workspace created"),
    (m::CLOUD_PAUSE, "cloud workspace paused"),
    (m::CLOUD_UNARCHIVE, "cloud workspace resumed"),
    (m::CLOUD_ARCHIVE, "cloud workspace archived"),
    (m::CLOUD_DELETE, "cloud workspace deleted"),
    (m::CLOUD_ACCOUNT_SIGN_IN, "account sign in started"),
    (m::CLOUD_ACCOUNT_SIGN_OUT, "account signed out"),
    (m::CLOUD_PROJECT_CONNECT, "cloud project connected"),
    (m::CLOUD_PROJECT_REMOVE, "cloud project removed"),
    (m::CLOUD_BILLING_CHECKOUT, "billing checkout started"),
    (m::CLOUD_BILLING_PORTAL, "billing portal opened"),
    (m::CLOUD_PREPAID_CHECKOUT, "prepaid checkout started"),
    (m::SLACK_CONNECT, "notification integration connected"),
    (m::SLACK_DISCONNECT, "notification integration disconnected"),
    (m::APPLY_UPDATE, "update applied"),
    (m::RESOLVE_RECOVERY, "recovery resolved"),
];

const COMMANDS: &[(&str, &str)] = &[
    ("run", "message submitted"),
    ("steer", "message submitted"),
    ("interrupt", "turn interrupted"),
    ("respondInput", "permission answered"),
    ("stopBackgroundTask", "background task stopped"),
];
const MUTATIONS: &[(&str, &str)] = &[
    ("createChat", "chat created"),
    ("createSpace", "project created"),
    ("deleteSpace", "project deleted"),
    ("deleteChat", "chat deleted"),
    ("forceDeleteChat", "chat deleted"),
    ("setChatArchived", "chat archived"),
    ("setChatCwd", "chat checkout changed"),
    ("changeSidebarPin", "sidebar pin changed"),
];
const PLUGINS: &[(&str, &str)] = &[
    ("connect", "plugin connected"),
    ("addServer", "plugin connected"),
    ("addLocalServer", "plugin connected"),
    ("removeServer", "plugin removed"),
];
const GIT_ACTIONS: &[&str] = &[
    "stage",
    "unstage",
    "commit",
    "push",
    "pull",
    "fetch",
    "createBranch",
    "switchBranch",
    "createPullRequest",
];
const REQUEST_ACTIONS: &[&str] = &[
    "edit",
    "addReviewers",
    "merge",
    "enableAutoMerge",
    "disableAutoMerge",
    "markReady",
    "markDraft",
    "close",
    "reopen",
];

fn lookup(rows: &[(&str, &'static str)], key: &str) -> Option<&'static str> {
    rows.iter()
        .find_map(|(candidate, name)| (*candidate == key).then_some(*name))
}

pub fn feature(method: &str, params: &Value) -> Option<&'static str> {
    match method {
        m::QUEUE_COMMAND => lookup(COMMANDS, params.pointer("/command/kind")?.as_str()?),
        m::MUTATE => lookup(MUTATIONS, params.get("op")?.as_str()?),
        m::PLUGINS => lookup(PLUGINS, params.get("action")?.as_str()?),
        _ => lookup(METHODS, method),
    }
}

pub fn valid_name(name: &str) -> bool {
    [METHODS, COMMANDS, MUTATIONS, PLUGINS]
        .into_iter()
        .any(|rows| rows.iter().any(|(_, candidate)| *candidate == name))
}

pub fn variant(method: &str, params: &Value) -> Option<&'static str> {
    let name = feature(method, params)?;
    let candidate = match method {
        m::GIT_WORKSPACE_ACTION => params.pointer("/operation/action")?.as_str()?,
        m::UPDATE_CHANGE_REQUEST | m::UPDATE_CHANGE_REQUEST_AT_HEAD => {
            params.pointer("/update/kind")?.as_str()?
        }
        m::MUTATE if name == "chat archived" => match params.get("archived")?.as_bool()? {
            true => "archive",
            false => "restore",
        },
        m::QUEUE_COMMAND => params.pointer("/command/kind")?.as_str()?,
        _ => return None,
    };
    variants(name)
        .iter()
        .copied()
        .find(|value| *value == candidate)
}

fn variants(feature: &str) -> &'static [&'static str] {
    match feature {
        "git action" => GIT_ACTIONS,
        "pull request updated" => REQUEST_ACTIONS,
        "chat archived" => &["archive", "restore"],
        "message submitted" => &["run", "steer"],
        "turn interrupted" => &["interrupt"],
        "permission answered" => &["respondInput"],
        "background task stopped" => &["stopBackgroundTask"],
        _ => &[],
    }
}

pub fn valid_variant(feature: &str, value: &str) -> bool {
    variants(feature).contains(&value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn actions_are_closed_and_do_not_read_private_values() {
        let params =
            json!({"operation": {"action": "commit", "message": "private"}, "cwd": "/private"});
        assert_eq!(
            feature(m::GIT_WORKSPACE_ACTION, &params),
            Some("git action")
        );
        assert_eq!(variant(m::GIT_WORKSPACE_ACTION, &params), Some("commit"));
        assert_eq!(
            variant(
                m::GIT_WORKSPACE_ACTION,
                &json!({"operation":{"action":"private"}})
            ),
            None
        );
        assert!(!valid_variant("chat created", "commit"));
    }

    #[test]
    fn reads_polling_keystrokes_and_background_refresh_do_not_count() {
        for method in [
            m::WATCH_CHAT_CONFIG,
            m::WRITE_TERMINAL,
            m::WATCH_CONNECTIVITY,
            m::WARM_WORKSPACE_SEARCH,
            m::LIST_CHANGE_REQUEST_PAGE,
            m::READ_WORKSPACE_FILE,
            m::RESIZE_TERMINAL,
            m::WATCH_TRANSFERS,
        ] {
            assert!(feature(method, &json!({})).is_none(), "{method}");
        }
    }

    #[test]
    fn command_and_lifecycle_variants_do_not_conflate_reverse_actions() {
        assert_eq!(
            variant(m::MUTATE, &json!({"op":"setChatArchived","archived":false})),
            Some("restore")
        );
        assert_eq!(
            variant(
                m::QUEUE_COMMAND,
                &json!({"command":{"kind":"steer","prompt":"private"}})
            ),
            Some("steer")
        );
        assert_eq!(
            variant(
                m::UPDATE_CHANGE_REQUEST,
                &json!({"update":{"kind":"enableAutoMerge","method":"squash"}})
            ),
            Some("enableAutoMerge")
        );
        assert!(feature(m::PLUGINS, &json!({"action":"private"})).is_none());
    }
}
