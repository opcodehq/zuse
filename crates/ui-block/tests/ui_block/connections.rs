//! Plugin connection cards and agent MCP server rows.

use crate::common::{rejects, root};
use zuse_ui_block::{Component, ServerStatus};

#[test]
fn connect_takes_a_plugin_and_an_optional_reason() {
    assert_eq!(
        root(r#"root = Connect(" linear ", "To file the bug you described")"#),
        Component::Connect {
            plugin: "linear".into(),
            reason: Some("To file the bug you described".into()),
        }
    );
    assert_eq!(
        root(r#"root = Connect("vidiq", "")"#),
        Component::Connect {
            plugin: "vidiq".into(),
            reason: None,
        }
    );
    rejects(r#"root = Connect("")"#, "Connect");
}

#[test]
fn mcp_command_cards_parse_with_live_plugins() {
    let card = root(
        r#"root = Card([zuse, docs, plugins], "MCP")
zuse = McpServer("zuse", "connected", "24 tools")
docs = McpServer("claude.ai Docs", "needs-auth")
plugins = ConnectedPlugins()"#,
    );
    let Component::Card { children, .. } = card else {
        panic!("expected a card");
    };
    assert_eq!(
        children[1],
        Component::McpServer {
            name: "claude.ai Docs".into(),
            status: ServerStatus::NeedsAuth,
            detail: None,
        }
    );
    assert_eq!(children[2], Component::ConnectedPlugins {});
    rejects(r#"root = McpServer("x", "asleep")"#, "status");
}

#[test]
fn new_components_round_trip_through_json() {
    let card = root(
        r#"root = Card([Connect("linear"), McpServer("x", "failed", "exited"), ConnectedPlugins()])"#,
    );
    let json = serde_json::to_value(&card).unwrap();
    assert_eq!(json["children"][1]["status"], "failed");
    assert_eq!(json["children"][2]["component"], "ConnectedPlugins");
    let back: Component = serde_json::from_value(json).unwrap();
    assert_eq!(back, card);
}

#[test]
fn machine_image_proposals_name_one_proposal_and_are_not_advertised() {
    assert_eq!(
        root(r#"root = MachineImageProposal(" msp_0123 ")"#),
        Component::MachineImageProposal {
            proposal: "msp_0123".into()
        }
    );
    rejects(r#"root = MachineImageProposal("")"#, "MachineImageProposal");
    assert!(!zuse_ui_block::tool_description().contains("MachineImageProposal"));
}
