//! Prepare a private invitation from the running staging desktop's actual engine.
use anyhow::{Context, ensure};
use serde_json::json;
use std::io::Write;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let assets = root
        .join("dist/web")
        .canonicalize()
        .context("Build browser assets first")?;
    ensure!(
        assets.join("index.html").is_file(),
        "Browser assets are incomplete"
    );
    let port = std::env::var("ZUSE_STAGING_IPC_PORT")
        .unwrap_or_else(|_| "28654".into())
        .parse::<u16>()?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    let client = loop {
        match zuse_rpc::connect_ws(&format!("ws://127.0.0.1:{port}")).await {
            Ok(client) => break client,
            Err(error) if tokio::time::Instant::now() >= deadline => return Err(error.into()),
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(200)).await,
        }
    };
    let identity = client.call("EngineInfo", json!({})).await?;
    if std::env::args().any(|arg| arg == "--verify") {
        let mut chats = client.subscribe_scoped("WatchChats", json!({})).await?;
        let snapshot = tokio::time::timeout(std::time::Duration::from_secs(15), chats.recv())
            .await?
            .context("Desktop chat stream closed")?;
        ensure!(
            snapshot
                .as_array()
                .is_some_and(|rows| rows.iter().any(|chat| {
                    chat["title"] == "Android ↔ Rust staging"
                        && chat["spaceId"] == "local-staging-worktree"
                        && chat["deviceId"] == identity["deviceId"]
                })),
            "The headed desktop did not observe Android's chat mutation"
        );
        println!("Verified: the headed desktop sees Android's encrypted chat mutation.");
        return Ok(());
    }
    let project_id = "local-staging-worktree";
    client
        .call(
            "Mutate",
            json!({"op":"createSpace","spaceId":project_id,"deviceId":identity["deviceId"],"path":root.canonicalize()?,"name":"Zuse staging","gitDetected":true}),
        )
        .await?;
    let mut status = client.call("BrowserAccessStatus", json!({})).await?;
    if status["running"] != true {
        status = client
            .call(
                "StartBrowserAccess",
                json!({"listen":"127.0.0.1:28655","assets":assets,"targetDeviceId":identity["deviceId"]}),
            )
            .await?;
    }
    let origin = status["origin"]
        .as_str()
        .context("Desktop listener did not return an origin")?;
    let grant = client
        .call("CreatePairing", json!({"label":"Android staging"}))
        .await?;
    let fragment = grant["fragment"].as_str().context("Missing invitation")?;
    let destination = root.join(".context/android-tools/staging-desktop-invitation.json");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&destination)?;
    file.write_all(serde_json::to_vec(&json!({"link":format!("{origin}/{fragment}"),"deviceId":identity["deviceId"],"peerId":grant["peer"]["id"],"origin":origin,"projectId":project_id}))?.as_slice())?;
    println!("Desktop remote listener: {origin}");
    println!("Private Android invitation saved; capability omitted from output.");
    Ok(())
}
