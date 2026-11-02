//! Offline shared-native-host measurement. Never sends model requests.
use async_trait::async_trait;
use std::{path::PathBuf, sync::Arc};
use zuse_harness::{
    native_tools::{NativeOptions, NativeTools, permission::RuntimeMode},
    processes::{ProcessNotice, ProcessNotifier, ProcessOptions},
    transport::NativeTransport,
    *,
};
struct Notices;
#[async_trait]
impl ProcessNotifier for Notices {
    async fn notify(&self, _: ProcessNotice) -> Result<(), Error> {
        Ok(())
    }
}
struct Application {
    _transport: Arc<NativeTransport>,
}
#[async_trait]
impl Host for Application {
    fn tools(&self) -> Tools {
        Tools::new()
    }
    async fn load(&self) -> Result<Option<Checkpoint>, Error> {
        Ok(None)
    }
    async fn save(&self, _: &Checkpoint) -> Result<(), Error> {
        Ok(())
    }
    async fn instructions(&self) -> Result<String, Error> {
        Ok("Offline host; do not invoke a model".into())
    }
    async fn model(&self, _: ModelInput) -> Result<ModelOutput, Error> {
        Err(Error::paused("Offline host never invokes models"))
    }
    async fn execute(
        &self,
        _: &str,
        _: serde_json::Value,
        _: ToolContext,
    ) -> Result<ToolOutput, Error> {
        Err(Error::Tool("No external tools".into()))
    }
    fn notify(&self, _: Event) {}
}
fn arg(name: &str, default: usize) -> usize {
    let args = std::env::args().collect::<Vec<_>>();
    args.windows(2)
        .find(|a| a[0] == name)
        .map_or(default, |a| a[1].parse().unwrap())
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let roots = arg("--roots", 1);
    if !(1..=20).contains(&roots) {
        return Err("roots must be 1..20".into());
    }
    let profile = tempfile::tempdir()?;
    let checkout = profile.path().join("checkout");
    std::fs::create_dir(&checkout)?;
    let transport = Arc::new(NativeTransport::new()?);
    let mut engines = vec![];
    for i in 0..roots {
        let app = Arc::new(Application {
            _transport: transport.clone(),
        });
        let cwd = checkout.clone();
        let locks = profile.path().join("locks");
        let root = format!("root-{i}");
        let engine = Engine::open_native_with(
            profile.path(),
            root.clone(),
            Options::new("offline-fixture"),
            move |directory| async move {
                let processes =
                    ProcessOptions::new(root, directory.join("artifacts"), Arc::new(Notices))?;
                let tools = Arc::new(
                    NativeTools::open(NativeOptions {
                        cwd,
                        lock_directory: locks,
                        processes,
                        runtime_mode: Arc::new(|| RuntimeMode::FullAccess),
                        callbacks: None,
                        ripgrep: PathBuf::from("rg"),
                        extensions: None,
                    })
                    .await?,
                );
                tools.attach(app)
            },
        )
        .await?;
        engines.push(engine);
    }
    use std::io::Write;
    println!("READY");
    std::io::stdout().flush()?;
    use tokio::io::AsyncReadExt;
    let mut command = [0];
    tokio::io::stdin().read_exact(&mut command).await?;
    for engine in &engines {
        engine.close().await?;
    }
    drop(engines);
    println!("CLOSED");
    std::io::stdout().flush()?;
    tokio::io::stdin().read_exact(&mut command).await?;
    Ok(())
}
