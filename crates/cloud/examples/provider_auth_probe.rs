//! Opt-in live verification of the production Rust Boxd wire adapter.
//! Unlike the SDK lifecycle benchmark, this checks our own token exchange/codec.
use anyhow::{Context, Result, ensure};
use zuse_cloud::providers::boxd::Boxd;

#[tokio::main]
async fn main() -> Result<()> {
    let mut provider = Boxd::new(
        std::env::var("BOXD_API_KEY").context("BOXD_API_KEY is required")?,
        std::env::var("BOXD_ORG").ok(),
    )
    .map_err(|_| anyhow::anyhow!("Invalid Boxd configuration"))?;
    if let Ok(base) = std::env::var("BOXD_BASE_URL") {
        provider = provider
            .with_base_url(&base)
            .map_err(|_| anyhow::anyhow!("Invalid Boxd endpoint"))?;
    }
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(
        args.is_empty()
            || args.len() == 4 && args[0] == "--machine" && args[2] == "--expected-name",
        "Use no arguments or --machine ID --expected-name NAME"
    );
    if !args.is_empty() {
        ensure!(
            args[3].starts_with("zuse-bench-"),
            "Only isolated benchmark machines can be probed"
        );
        let machine = provider
            .inspect(&args[1])
            .await
            .map_err(|error| anyhow::anyhow!("Rust Boxd inspect failed: {error:?}"))?
            .context("Benchmark machine not found")?;
        ensure!(
            machine.id == args[1] && machine.name == args[3],
            "Benchmark machine identity mismatch"
        );
        ensure!(
            machine.state == zuse_cloud::providers::MachineState::Running,
            "Benchmark machine is not running"
        );
        let result = provider
            .exec(&machine.id, "printf zuse-rust-wire-ok")
            .await
            .map_err(|error| anyhow::anyhow!("Rust Boxd exec failed: {error:?}"))?;
        ensure!(
            result.exit_code == 0
                && result.stdout == b"zuse-rust-wire-ok"
                && result.stderr.is_empty(),
            "Rust Boxd exec round trip failed"
        );
        println!(
            "{}",
            serde_json::json!({"provider":"boxd", "rustAdapterAuthenticated":true,"machineIdentityVerified":true,"execRoundTripVerified":true,"machineLifecycleMutations":0})
        );
        return Ok(());
    }
    let name = format!("zuse-read-only-probe-{}", uuid::Uuid::new_v4());
    let started = std::time::Instant::now();
    let machine = provider
        .inspect(&name)
        .await
        .map_err(|error| anyhow::anyhow!("Rust Boxd lookup failed: {error:?}"))?;
    ensure!(machine.is_none(), "Unexpected probe identity collision");
    println!(
        "{}",
        serde_json::json!({
            "provider":"boxd", "rustAdapterAuthenticated":true,
            "tokenExchangeAndLookupMs":started.elapsed().as_millis(),
            "mutations":0,
        })
    );
    Ok(())
}
