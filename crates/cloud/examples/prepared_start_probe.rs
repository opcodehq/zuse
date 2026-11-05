//! Opt-in provider baseline for the actual managed image. No account enrollment.
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use zuse_cloud::providers::{
    contract::{Allocation, OperationState, Provider},
    registry::Registry,
};
use zuse_protocol::cloud_computers::ProviderId;

async fn ready(provider: &dyn Provider, machine: &str) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if let Ok(result) = provider.execute(machine, "test -x /usr/local/bin/zuse-cloud-runtime && /usr/local/bin/zuse-cloud-runtime --version").await {
            if result.exit_code == 0 { return Ok(()); }
        }
        ensure!(
            Instant::now() < deadline,
            "prepared_machine_readiness_timeout"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn sample(
    provider: Arc<dyn Provider>,
    artifact: String,
    path: PathBuf,
    index: usize,
) -> Result<Value> {
    let name = format!("zuse-bench-prepared-{}", uuid::Uuid::new_v4().simple());
    // Retain the exact create identity even if the network outcome is unknown.
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&json!({"name":name,"artifact":artifact,"phase":"creating"}))?,
    )?;
    let started = Instant::now();
    let created = provider
        .allocate(&Allocation {
            artifact: &artifact,
            artifact_binding: None,
            name: &name,
            operation_id: &name,
            size: "default",
            lifetime_seconds: 3600,
        })
        .await;
    let create_ms = started.elapsed().as_millis() as u64;
    let machine = match created {
        Ok(machine) => machine,
        Err(_) => {
            // Reconcile the original name; never submit a replacement create.
            if let Some(machine) = provider
                .find_allocation(&name)
                .await
                .map_err(|_| anyhow::anyhow!("probe_create_outcome_unknown"))?
            {
                machine
            } else {
                anyhow::bail!("probe_create_outcome_unknown; original name retained in receipt");
            }
        }
    };
    ensure!(machine.name == name, "probe_machine_identity_mismatch");
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(
            &json!({"name":name,"machineId":machine.id,"artifact":artifact,"phase":"observing"}),
        )?,
    )?;
    let observed = ready(provider.as_ref(), &machine.id).await;
    let ready_ms = started.elapsed().as_millis() as u64;
    let cleanup = provider.delete(&machine.id).await;
    let mut deleted = false;
    if let Ok(receipt) = cleanup {
        let until = Instant::now() + Duration::from_secs(45);
        while Instant::now() < until {
            if matches!(
                provider.operation(&receipt).await,
                Ok(OperationState::Completed)
            ) {
                deleted = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    let row = json!({"index":index,"name":name,"machineId":machine.id,"artifact":artifact,
        "createResponseMs":create_ms,"createToInstalledRuntimeCommandMs":ready_ms,
        "ready":observed.is_ok(),"deleted":deleted,"includesAccountEnrollment":false,"includesAgentPrompt":false});
    std::fs::write(&path, serde_json::to_vec_pretty(&row)?)?;
    ensure!(
        deleted,
        "probe_cleanup_not_confirmed; original identity retained in receipt"
    );
    observed?;
    Ok(row)
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 4
            && args[0] == "--live"
            && args[1] == "--snapshot"
            && args[3].parse::<usize>().is_ok(),
        "Usage: prepared_start_probe --live --snapshot SNAPSHOT SAMPLES (1..30)"
    );
    let samples = args[3].parse::<usize>()?;
    ensure!((1..=30).contains(&samples), "invalid_probe_sample_count");
    let root = PathBuf::from(format!(
        ".context/prepared-start-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
    }
    let provider = Registry::from_environment()
        .get(ProviderId::Boxd)
        .ok_or_else(|| anyhow::anyhow!("boxd_configuration_required"))?;
    let mut rows = Vec::new();
    for i in 0..samples {
        let row = sample(
            provider.clone(),
            args[2].clone(),
            root.join(format!("sample-{i}.json")),
            i,
        )
        .await?;
        println!("{}", row);
        rows.push(row);
    }
    let mut durations: Vec<_> = rows
        .iter()
        .filter_map(|r| r["createToInstalledRuntimeCommandMs"].as_u64())
        .collect();
    durations.sort_unstable();
    println!(
        "{}",
        json!({"measurement":"provider-command-readiness","samples":samples,
        "p50Ms":durations[(samples-1)/2],"p95Ms":durations[(samples*95).div_ceil(100)-1],"receiptDirectory":root,
        "applicationStartupVerified":false})
    );
    Ok(())
}
