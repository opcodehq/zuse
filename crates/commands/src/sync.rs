//! `zuse sync`: per-room sync state from the running engine.

/// `zuse sync`: dial the running engine's IPC and print per-room sync state.
/// The introspection surface every 2026-08 incident was missing — "is this
/// device's workspace room actually receiving?" as a one-liner.
pub async fn run(ipc_port: u16) -> anyhow::Result<()> {
    let client = zuse_rpc::connect_ws(&format!("ws://127.0.0.1:{ipc_port}"))
        .await
        .map_err(|e| {
            anyhow::anyhow!("no engine listening on 127.0.0.1:{ipc_port} ({e}) — is zuse running?")
        })?;
    let status = client
        .call(zuse_rpc::methods::SYNC_STATUS, serde_json::json!({}))
        .await
        .map_err(|e| anyhow::anyhow!("SyncStatus failed: {e}"))?;
    let now = status.get("nowMs").and_then(|v| v.as_i64()).unwrap_or(0);
    let age = |ms: i64| -> String {
        if ms <= 0 {
            return "never".into();
        }
        let s = (now - ms).max(0) / 1000;
        if s >= 3600 {
            format!("{}h{}m ago", s / 3600, (s % 3600) / 60)
        } else if s >= 60 {
            format!("{}m{}s ago", s / 60, s % 60)
        } else {
            format!("{s}s ago")
        }
    };
    let room_line = |room: Option<&serde_json::Value>| -> String {
        let Some(room) = room else {
            return "no room (dialing or edge-less)".into();
        };
        let get = |k: &str| room.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
        // REJECTED is loud and only shown when nonzero: rejected writes with
        // a fresh-looking room is exactly the latched-session wedge
        // (2026-08-04) this readout previously masked.
        let rejected = get("rejected");
        format!(
            "{} pushed {} · acked {} · rejoins {} probes {} resyncs {} drops {}{}",
            if room.get("connected").and_then(|v| v.as_bool()) == Some(true) {
                "connected ·"
            } else {
                "DISCONNECTED ·"
            },
            age(get("lastPushedMs")),
            age(get("lastAckMs")),
            get("rejoins"),
            get("probes"),
            get("fullResyncs"),
            get("disconnects"),
            if rejected > 0 {
                format!(" REJECTED {rejected}")
            } else {
                String::new()
            },
        )
    };
    println!(
        "Device:    {}",
        status
            .get("deviceId")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
    );
    println!(
        "Workspace: {}",
        room_line(status.get("workspace").filter(|v| !v.is_null()))
    );
    let chats = status
        .get("chats")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if chats.is_empty() {
        println!("Chats:     none open");
    }
    // Chat rooms speak chat2: cursor/head tell "am I caught up?", pending
    // tells "did my writes leave?", resets/rejected are the loud tells.
    let chat_line = |room: Option<&serde_json::Value>| -> String {
        let Some(room) = room else {
            return "no room (dialing or edge-less)".into();
        };
        let get = |k: &str| room.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let resets = get("serverResets");
        let rejected = get("rejected");
        format!(
            "{} cursor {}/{} · pending {} · rows {} ({}KB) · rejoins {} drops {}{}{}",
            if room.get("connected").and_then(|v| v.as_bool()) == Some(true) {
                "connected ·"
            } else {
                "DISCONNECTED ·"
            },
            get("cursor"),
            get("headSeq"),
            get("pendingPushes"),
            get("rowCount"),
            get("rowBytes") / 1024,
            get("rejoins"),
            get("disconnects"),
            if resets > 0 {
                format!(" RESETS {resets}")
            } else {
                String::new()
            },
            if rejected > 0 {
                format!(" REJECTED {rejected}")
            } else {
                String::new()
            },
        )
    };
    for chat in &chats {
        println!(
            "Chat {}: {}",
            chat.get("chatId")
                .and_then(|v| v.as_str())
                .map(|s| &s[..s.len().min(8)])
                .unwrap_or("?"),
            chat_line(chat.get("room").filter(|v| !v.is_null()))
        );
    }
    Ok(())
}
