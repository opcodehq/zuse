//! One durable image step per lease; provider snapshots are promoted only after read-back verification.
use super::*;
use crate::providers::{
    Machine, MachineState, ProviderError,
    boxd::{Boxd, SnapshotCapture, SnapshotState},
};
use async_trait::async_trait;
use github::{Grant, GrantIssuer};
use sqlx::Row;
#[derive(Default)]
pub struct Preparation {
    pub running: bool,
    pub failed: bool,
    pub manifest: Option<ImageManifest>,
    pub log: String,
    pub phase: String,
}
#[async_trait]
pub trait ImageProvider: Send + Sync {
    async fn find(&self, id: &str) -> Result<Option<Machine>, ProviderError>;
    async fn allocate(&self, snapshot: &str, name: &str) -> Result<Machine, ProviderError>;
    async fn provenance(
        &self,
        machine: &str,
        snapshot: &str,
        version: u64,
    ) -> Result<(), ProviderError>;
    async fn preparation(
        &self,
        machine: &str,
        plan: &ImagePlan,
    ) -> Result<Preparation, ProviderError>;
    async fn launch(
        &self,
        machine: &str,
        plan: &ImagePlan,
        grants: &[Grant],
    ) -> Result<(), ProviderError>;
    async fn verify(&self, machine: &str, plan: &ImagePlan)
    -> Result<ImageManifest, ProviderError>;
    async fn snapshot(
        &self,
        name: &str,
    ) -> Result<Option<crate::providers::boxd::Snapshot>, ProviderError>;
    async fn capture(&self, machine: &str, name: &str) -> Result<SnapshotCapture, ProviderError>;
    async fn destroy(&self, id: &str, name: &str) -> Result<(), ProviderError>;
    async fn delete_snapshot(&self, id: &str) -> Result<(), ProviderError>;
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
#[async_trait]
impl ImageProvider for Boxd {
    async fn find(&self, id: &str) -> Result<Option<Machine>, ProviderError> {
        self.inspect(id).await
    }
    async fn allocate(&self, snapshot: &str, name: &str) -> Result<Machine, ProviderError> {
        self.create_from_snapshot(snapshot, name, true).await
    }
    async fn provenance(
        &self,
        machine: &str,
        snapshot: &str,
        version: u64,
    ) -> Result<(), ProviderError> {
        self.verify_snapshot_source(machine, snapshot, version)
            .await
    }
    async fn snapshot(
        &self,
        name: &str,
    ) -> Result<Option<crate::providers::boxd::Snapshot>, ProviderError> {
        self.inspect_snapshot(name).await
    }
    async fn capture(&self, machine: &str, name: &str) -> Result<SnapshotCapture, ProviderError> {
        self.create_snapshot(machine, name).await
    }
    async fn delete_snapshot(&self, id: &str) -> Result<(), ProviderError> {
        Boxd::delete_snapshot(self, id).await
    }
    async fn destroy(&self, id: &str, name: &str) -> Result<(), ProviderError> {
        if let Some(machine) = self.inspect(id).await? {
            if machine.id != id || machine.name != name {
                return Err(ProviderError::Conflict);
            }
            self.destroy(id).await?;
        }
        Ok(())
    }
    async fn preparation(
        &self,
        machine: &str,
        plan: &ImagePlan,
    ) -> Result<Preparation, ProviderError> {
        let dir = quote(&plan.status_directory.to_string_lossy());
        let source = r#"import json,sys,os,fcntl
p=sys.argv[1];out={'running':False,'failed':False,'phase':'queued','log':''}
try:
 f=open(p+'/launch.lock','a');fcntl.flock(f,fcntl.LOCK_EX|fcntl.LOCK_NB)
except BlockingIOError:out['running']=True
try:
 with open(p+'/exit-code') as f:code=int(f.read().strip())
 out['failed']=code!=0
 if code==0:
  with open(p+'/manifest.json') as f:out['manifest']=json.load(f)
except FileNotFoundError:pass
try:
 with open(p+'/progress.json') as f:out['phase']=json.load(f)['phase']
except FileNotFoundError:pass
try:
 with open(p+'/build.log') as f:out['log']=f.read(65536)
except FileNotFoundError:pass
print(json.dumps(out))"#;
        let command = format!(
            "sudo -n install -d -o zuse -g zuse -m 700 {dir}\nsudo -n -u zuse python3 -c {} {dir}",
            quote(source)
        );
        let result = self.exec(machine, &command).await?;
        if result.exit_code != 0 {
            return Err(ProviderError::Unavailable);
        }
        let value: Value =
            serde_json::from_slice(&result.stdout).map_err(|_| ProviderError::InvalidResponse)?;
        let manifest = value
            .get("manifest")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .map_err(|_| ProviderError::InvalidResponse)?;
        Ok(Preparation {
            running: value["running"] == true,
            failed: value["failed"] == true,
            manifest,
            log: value["log"].as_str().unwrap_or("").into(),
            phase: value["phase"].as_str().unwrap_or("preparing").into(),
        })
    }
    async fn verify(
        &self,
        machine: &str,
        plan: &ImagePlan,
    ) -> Result<ImageManifest, ProviderError> {
        let path = quote(&format!("{}/plan.json", plan.status_directory.display()));
        // The guest's completed-plan path reopens and fscks each cached repository,
        // validates all pinned refs and checks that no credentials remain.
        let result=self.exec(machine,&format!("sudo -n -u zuse env -i HOME=/home/zuse PATH=/usr/local/bin:/usr/bin:/bin /usr/local/bin/zuse-cloud-runtime prepare-image --plan {path}")).await?;
        if result.exit_code != 0 {
            return Err(ProviderError::Rejected);
        }
        serde_json::from_slice(&result.stdout).map_err(|_| ProviderError::InvalidResponse)
    }
    async fn launch(
        &self,
        machine: &str,
        plan: &ImagePlan,
        grants: &[Grant],
    ) -> Result<(), ProviderError> {
        let attempt = uuid::Uuid::new_v4().simple().to_string();
        let upload_root = format!("{}/uploads", plan.status_directory.display());
        let init = self
            .exec(
                machine,
                &format!(
                    "sudo -n install -d -o root -g root -m 700 {}",
                    quote(&upload_root)
                ),
            )
            .await?;
        if init.exit_code != 0 {
            return Err(ProviderError::Unavailable);
        }
        let stage = format!("{upload_root}/{attempt}-plan");
        let dir = plan.status_directory.to_string_lossy();
        let script = format!("{upload_root}/{attempt}-runner.sh");
        self.upload(
            machine,
            &stage,
            &serde_json::to_vec(plan).map_err(|_| ProviderError::Rejected)?,
        )
        .await?;
        let mut installs = String::new();
        let mut stages = vec![stage.clone(), script.clone()];
        for grant in grants {
            if grant.installation <= 0 {
                return Err(ProviderError::Rejected);
            }
            let staged = format!("{upload_root}/{attempt}-token-{}", grant.installation);
            // upload uses a private temporary destination; every attempt owns distinct paths.
            self.upload(machine, &staged, grant.token.as_bytes())
                .await?;
            installs.push_str(&format!(
                "sudo -n install -o zuse -g zuse -m 600 {} {}\n",
                quote(&staged),
                quote(&format!(
                    "/run/zuse-secrets/github-installation-{}",
                    grant.installation
                ))
            ));
            stages.push(staged);
        }
        let cleanup = stages
            .iter()
            .map(|s| quote(s))
            .collect::<Vec<_>>()
            .join(" ");
        let runner = format!(
            r#"set -eu
held=0
finish() {{
 code=$?
 rm -f -- {cleanup}
 if [ "$held" = 1 ]; then rm -f -- {upload_root}/*; fi
 if [ "$held" = 1 ]; then
  rm -f -- /run/zuse-secrets/github-installation-*
  printf '%s\n' "$code" >{dir}/exit-code.next
  mv {dir}/exit-code.next {dir}/exit-code
 fi
}}
trap finish EXIT
exec 9>{dir}/launch.lock
flock -n 9 || exit 0
[ ! -f {dir}/exit-code ] || exit 0
held=1
sudo -n -u zuse env -i HOME=/home/zuse PATH=/usr/local/bin:/usr/bin:/bin /bin/sh -c 'test -x /usr/local/bin/zuse-cloud-runtime && command -v claude >/dev/null && command -v git >/dev/null'
sudo -n install -d -o zuse -g zuse -m 700 /run/zuse-secrets /home/repos
{installs}sudo -n install -o zuse -g zuse -m 600 {stage} {dir}/plan.json
sudo -n -u zuse env -i HOME=/home/zuse PATH=/usr/local/bin:/usr/bin:/bin /usr/local/bin/zuse-cloud-runtime prepare-image --plan {dir}/plan.json >{dir}/result.json 2>{dir}/diagnostic.log
"#,
            upload_root = quote(&upload_root),
            dir = quote(&dir),
            stage = quote(&stage)
        );
        self.upload(machine, &script, runner.as_bytes()).await?;
        let result = self
            .exec(
                machine,
                &format!(
                    "sudo -n chmod 700 {}\nsudo -n nohup /bin/bash {} >/dev/null 2>&1 </dev/null &",
                    quote(&script),
                    quote(&script)
                ),
            )
            .await?;
        if result.exit_code != 0 {
            return Err(ProviderError::Unavailable);
        }
        Ok(())
    }
}
pub(super) fn provider_error(error: ProviderError) -> ApiError {
    ApiError::unavailable(match error {
        ProviderError::Rejected => "image_provider_rejected",
        ProviderError::Conflict => "image_provider_identity_conflict",
        ProviderError::OutcomeUnknown => "image_provider_outcome_unknown",
        ProviderError::NotFound => "image_provider_resource_missing",
        _ => "image_provider_unavailable",
    })
}
impl ImageControl {
    pub async fn work_once(
        &self,
        provider: &dyn ImageProvider,
        issuer: Option<&dyn GrantIssuer>,
    ) -> Result<bool, ApiError> {
        let deletion_work = self.delete_once(provider).await?;
        let at = now();
        let token = uuid::Uuid::new_v4().to_string();
        let accounts = self.accounts.iter().cloned().collect::<Vec<_>>();
        let row=sqlx::query("WITH candidate AS(SELECT j.job_id FROM zuse_rust_jobs j JOIN zuse_rust_image_operations op ON op.build_id=j.resource_id JOIN zuse_rust_control_owners o ON o.resource_kind='image-build' AND o.resource_id=j.resource_id AND o.backend='rust' AND o.generation=j.resource_generation WHERE j.kind='image-build' AND EXISTS(SELECT 1 FROM zuse_rust_control_owners a WHERE a.resource_kind='project-catalog' AND a.resource_id=op.account_id AND a.backend='rust') AND EXISTS(SELECT 1 FROM zuse_rust_control_owners a WHERE a.resource_kind='image-catalog' AND a.resource_id=op.account_id AND a.backend='rust') AND op.account_id=ANY($1) AND j.state IN ('pending','running') AND j.due_at<=$2 AND (j.lease_expires_at IS NULL OR j.lease_expires_at<=$2) ORDER BY j.due_at,j.created_at FOR UPDATE OF j SKIP LOCKED LIMIT 1) UPDATE zuse_rust_jobs j SET state='running',lease_token=$3,lease_expires_at=$2+300000,attempts=attempts+1,updated_at=$2 FROM candidate WHERE j.job_id=candidate.job_id RETURNING j.*").bind(accounts).bind(at).bind(&token).fetch_optional(&self.pool).await?;
        let Some(row) = row else {
            return Ok(deletion_work);
        };
        let job = jobs::Job {
            id: row.try_get("job_id")?,
            kind: "image-build".into(),
            resource_id: row.try_get("resource_id")?,
            generation: row.try_get("resource_generation")?,
            payload: row.try_get("payload")?,
            lease_token: token,
            attempts: row.try_get("attempts")?,
        };
        let future = self.step(&job, provider, issuer);
        tokio::pin!(future);
        let result = loop {
            tokio::select! {result=&mut future=>break result,_=tokio::time::sleep(std::time::Duration::from_secs(30))=>{
                if !jobs::extend(&self.pool,&job,"image-build",now(),300000).await?{return Err(ApiError::conflict("image_build_fenced"));}
            }}
        };
        match result {
            Ok(complete) => {
                if complete {
                    jobs::finish(&self.pool, &job, "image-build", now()).await?;
                } else {
                    self.defer(&job, None).await?;
                }
            }
            Err(error) => {
                self.defer(&job, Some(error.code)).await?;
                return Err(error);
            }
        }
        Ok(true)
    }
    async fn defer(&self, job: &jobs::Job, error: Option<&str>) -> Result<(), ApiError> {
        sqlx::query("UPDATE zuse_rust_jobs SET state='pending',due_at=$3,lease_token=NULL,lease_expires_at=NULL,last_error_code=$4,updated_at=$5 WHERE job_id=$1 AND lease_token=$2 AND state='running'").bind(&job.id).bind(&job.lease_token).bind(now()+2000).bind(error).bind(now()).execute(&self.pool).await?;
        Ok(())
    }
    async fn step(
        &self,
        job: &jobs::Job,
        provider: &dyn ImageProvider,
        issuer: Option<&dyn GrantIssuer>,
    ) -> Result<bool, ApiError> {
        let record: Value =
            sqlx::query_scalar("SELECT record FROM zuse_rust_image_operations WHERE build_id=$1")
                .bind(&job.resource_id)
                .fetch_one(&self.pool)
                .await?;
        let mut op: Operation = serde_json::from_value(record).map_err(|_| unavailable())?;
        let mut tx = self.owned(&op.account).await?;
        let locked: bool =
            sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended($1,68139))")
                .bind(&op.build)
                .fetch_one(&mut *tx)
                .await?;
        if !locked {
            return Err(ApiError::conflict("image_build_busy"));
        }
        let owner:Option<String>=sqlx::query_scalar("SELECT backend FROM zuse_rust_control_owners WHERE resource_kind='image-build' AND resource_id=$1 AND generation=$2 FOR SHARE").bind(&op.build).bind(job.generation).fetch_optional(&mut *tx).await?;
        let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3)").bind(&job.id).bind(&job.lease_token).bind(now()).fetch_one(&mut *tx).await?;
        if owner.as_deref() != Some("rust")
            || !live
            || op.build != job.resource_id
            || op.version != 1
        {
            return Err(ApiError::conflict("image_build_fenced"));
        }
        // Reload only after the operation guard: another delivery may have advanced it.
        let record: Value = sqlx::query_scalar(
            "SELECT record FROM zuse_rust_image_operations WHERE build_id=$1 FOR UPDATE",
        )
        .bind(&op.build)
        .fetch_one(&mut *tx)
        .await?;
        let account = op.account.clone();
        op = serde_json::from_value(record).map_err(|_| unavailable())?;
        if op.account != account || op.build != job.resource_id || op.version != 1 {
            return Err(ApiError::conflict("image_build_fenced"));
        }
        if op.phase == "complete" {
            tx.commit().await?;
            return Ok(true);
        }
        if now() - op.created > 30 * 60_000
            && op.failure.is_none()
            && !matches!(op.phase.as_str(), "retire-images" | "complete")
        {
            op.failure = Some("image_build_timeout".into());
            op.phase = "cleanup".into();
            // Publish the timeout before provider cleanup, which may itself be unavailable.
            sqlx::query(
                "UPDATE zuse_rust_image_operations SET record=$2,updated_at=$3 WHERE build_id=$1",
            )
            .bind(&op.build)
            .bind(serde_json::to_value(&op).map_err(|_| unavailable())?)
            .bind(now())
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE api_cloud_project_builds SET state='failed',last_error_code='image_build_timeout',settings=settings||'{\"rustImagePhase\":\"cleanup\"}'::jsonb,updated_at=$2 WHERE build_id=$1").bind(&op.build).bind(now()).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(false);
        }
        let mut log = None;
        sqlx::query("SAVEPOINT image_step")
            .execute(&mut *tx)
            .await?;
        let action:Result<(),ApiError>=async{
            if !matches!(op.phase.as_str(),"cleanup"|"promote"|"retire-images") {
                if op.resources.is_none() {
                    let base=provider.snapshot(&op.config.base_snapshot).await.map_err(provider_error)?.ok_or_else(||ApiError::conflict("image_base_snapshot_missing"))?;
                    if base.id!=op.config.base_snapshot||base.version!=op.config.base_version||base.state!=SnapshotState::Ready||base.vcpu==0||base.memory_bytes==0{return Err(ApiError::conflict("image_base_snapshot_mismatch"));}
                    op.resources=Some((base.vcpu,base.memory_bytes.div_ceil(1024*1024)));
                }
                if op.phase=="verify"&&op.verifier_started_at.is_none(){op.verifier_started_at=Some(now());}
                reserve(&mut tx,&op,now()).await?;
            }
            match op.phase.as_str(){
                "allocate"=>{
                    if op.repositories.iter().any(|r|r.installation.is_some())&&issuer.is_none(){return Err(ApiError::conflict("image_github_grants_not_configured"));}
                    crate::billing::require_capacity(&mut tx,&op.account,now()).await?;
                    let machine=if let Some(id)=&op.machine{provider.find(id).await.map_err(provider_error)?}else{provider.find(&op.machine_name).await.map_err(provider_error)?};
                    let machine=match machine{Some(m)=>m,None=>provider.allocate(&op.config.base_snapshot,&op.machine_name).await.map_err(provider_error)?};
                    if machine.name!=op.machine_name{return Err(ApiError::conflict("image_provider_identity_conflict"));}op.machine=Some(machine.id.clone());
                    if machine.state==MachineState::Running{provider.provenance(&machine.id,&op.config.base_snapshot,op.config.base_version).await.map_err(provider_error)?;op.phase="prepare".into();}
                },
                "prepare"=>{
                    let machine=op.machine.as_deref().ok_or_else(unavailable)?;let preparation=provider.preparation(machine,&op.plan).await.map_err(provider_error)?;log=Some(preparation.log);
                    if preparation.failed{return Err(ApiError::conflict("image_preparation_failed"));}
                    if let Some(manifest)=preparation.manifest{validate_manifest(&op,&manifest)?;op.manifest=Some(manifest);op.phase="capture".into();}
                    else if !preparation.running {
                        if now()>op.approved_until{return Err(ApiError::conflict("image_repository_authorization_expired"));}
                        for repository in &op.repositories {
                            if let Some(installation)=repository.installation {
                                let row:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(i) FROM api_cloud_github_installations i WHERE account_id=$1 AND installation_id=$2 AND suspended=false FOR SHARE").bind(&op.account).bind(installation).fetch_optional(&mut *tx).await?;
                                let row=row.ok_or_else(||ApiError::forbidden("cloud_github_repository_access_required"))?;
                                let name=text(&repository.row,"repository_identity")?.strip_prefix("github.com/").ok_or_else(unavailable)?;
                                if row["allowed_repositories"].as_array().is_some_and(|items|!items.iter().any(|n|n.as_str().is_some_and(|n|n.eq_ignore_ascii_case(name)))){return Err(ApiError::forbidden("cloud_github_repository_access_required"));}
                            }
                        }
                        let grants=if let Some(issuer)=issuer{issuer.mint(&op).await?}else{vec![]};
                        provider.launch(machine,&op.plan,&grants).await.map_err(provider_error)?;
                    }
                },
                "capture"=>{
                    match provider.snapshot(&op.snapshot_name).await.map_err(provider_error)?{
                        Some(snapshot)=>{
                            if snapshot.name!=op.snapshot_name||op.snapshot.as_ref().is_some_and(|id|id!=&snapshot.id)||op.snapshot_version.is_some_and(|v|snapshot.version>0&&v!=snapshot.version){return Err(ApiError::conflict("image_snapshot_identity_mismatch"));}
                            if snapshot.state==SnapshotState::Failed{return Err(ApiError::conflict("image_snapshot_failed"));}
                            op.snapshot=Some(snapshot.id);if snapshot.version>0{op.snapshot_version=Some(snapshot.version);}
                            if snapshot.state==SnapshotState::Ready{op.phase="verify".into();}
                        },None=>{let capture=provider.capture(op.machine.as_deref().ok_or_else(unavailable)?,&op.snapshot_name).await.map_err(provider_error)?;op.snapshot=Some(capture.id);if capture.version>0{op.snapshot_version=Some(capture.version);}}
                    }
                },
                "verify"=>{
                    let snapshot=op.snapshot.as_deref().ok_or_else(unavailable)?;let version=op.snapshot_version.ok_or_else(unavailable)?;
                    let machine=if let Some(id)=&op.verifier{provider.find(id).await.map_err(provider_error)?}else{provider.find(&op.verify_name).await.map_err(provider_error)?};
                    let machine=match machine{Some(m)=>m,None=>provider.allocate(snapshot,&op.verify_name).await.map_err(provider_error)?};
                    if machine.name!=op.verify_name{return Err(ApiError::conflict("image_provider_identity_conflict"));}op.verifier=Some(machine.id.clone());
                    if machine.state==MachineState::Running{
                        provider.provenance(&machine.id,snapshot,version).await.map_err(provider_error)?;
                        let manifest=provider.verify(&machine.id,&op.plan).await.map_err(provider_error)?;validate_manifest(&op,&manifest)?;
                        if serde_json::to_value(&manifest).ok()!=serde_json::to_value(&op.manifest).ok(){return Err(ApiError::conflict("image_snapshot_readback_mismatch"));}
                        op.verified=true;op.phase="cleanup".into();
                    }
                },
                "cleanup"=>{
                    // Recover allocations whose create response was lost before persisting their IDs.
                    if op.machine.is_none(){op.machine=provider.find(&op.machine_name).await.map_err(provider_error)?.map(|m|m.id);}
                    if op.verifier.is_none(){op.verifier=provider.find(&op.verify_name).await.map_err(provider_error)?.map(|m|m.id);}
                    if let Some(id)=&op.verifier{observe(&mut tx,&op,id).await?;provider.destroy(id,&op.verify_name).await.map_err(provider_error)?;op.verifier=None;}
                    if let Some(id)=&op.machine{observe(&mut tx,&op,id).await?;provider.destroy(id,&op.machine_name).await.map_err(provider_error)?;}
                    if op.failure.is_some(){if let Some(snapshot)=provider.snapshot(&op.snapshot_name).await.map_err(provider_error)?{provider.delete_snapshot(&snapshot.id).await.map_err(provider_error)?;}op.phase="complete".into();}
                    else if op.verified{op.phase="promote".into();}else{return Err(ApiError::conflict("image_not_verified"));}
                },
                "promote"=>{
                    if !op.verified||op.manifest.is_none()||op.failure.is_some(){return Err(ApiError::conflict("image_not_verified"));}
                    snapshot_guard(&mut tx,&op.account).await?;
                    let retained_at=now();
                    sqlx::query("UPDATE api_cloud_snapshots SET state='deleting',record=record||$2 WHERE account_id=$1 AND provider='box' AND state='retained'").bind(&op.account).bind(json!({"state":"deleting","stoppedAtMs":retained_at,"deletionReason":"superseded","nextAttemptAtMs":retained_at})).execute(&mut *tx).await?;
                    let id=op.snapshot.as_deref().ok_or_else(unavailable)?;
                    let record=json!({"snapshotId":id,"provider":"box","accountId":op.account,"buildId":op.build,"state":"retained","createdAtMs":op.created,"retainedAtMs":retained_at,"checkpointAtMs":retained_at,"remainder":0,"attempts":0,"nextAttemptAtMs":retained_at});
                    let inserted=sqlx::query("INSERT INTO api_cloud_snapshots(provider,snapshot_id,account_id,build_id,state,created_at,record) VALUES('box',$1,$2,$3,'retained',$4,$5) ON CONFLICT(provider,snapshot_id) DO UPDATE SET state=EXCLUDED.state,record=EXCLUDED.record WHERE api_cloud_snapshots.account_id=EXCLUDED.account_id AND api_cloud_snapshots.build_id=EXCLUDED.build_id").bind(id).bind(&op.account).bind(&op.build).bind(op.created).bind(record).execute(&mut *tx).await?;
                    if inserted.rows_affected()!=1{return Err(ApiError::conflict("image_snapshot_identity_mismatch"));}
                    let manifest=op.manifest.as_ref().unwrap();let layouts=manifest.repositories.iter().map(|r|(r.project_id.clone(),json!({"cachePath":r.path,"sourceCommit":r.source_commit}))).collect::<serde_json::Map<_,_>>();
                    sqlx::query("UPDATE api_cloud_project_builds SET state='ready',snapshot_id=$2,provider_sandbox_id=$3,settings=settings||$4,last_error_code=NULL,revision=revision+1,updated_at=$5 WHERE build_id=$1 AND account_id=$6").bind(&op.build).bind(&op.snapshot).bind(&op.machine).bind(json!({"runtimeRpcProtocol":"zuse-rpc-v1","rustRuntimeStorageFormat":zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT,"rustSessionCommandVersion":1,"rustRuntimeRepositoryLayouts":layouts,"providerAuthDeliveryVersion":1,"rustImagePhase":"complete","rustLaunchAssetsVersion":manifest.rust_launch_assets_version,"rustLaunchModelConfigVersion":manifest.rust_launch_model_config_version,"rustSlackVersion":manifest.rust_slack_version})).bind(now()).bind(&op.account).execute(&mut *tx).await?;
                    for repo in &op.repositories{sqlx::query("UPDATE api_cloud_projects SET state='ready',last_error_code=NULL,updated_at=$3 WHERE project_id=$1 AND account_id=$2 AND included=true AND configuration_digest=$4").bind(text(&repo.row,"project_id")?).bind(&op.account).bind(now()).bind(text(&repo.row,"configuration_digest")?).execute(&mut *tx).await?;}
                    op.phase="retire-images".into();
                },
                "retire-images"=>{
                    snapshot_guard(&mut tx,&op.account).await?;
                    let rows:Vec<Value>=sqlx::query_scalar("SELECT record FROM api_cloud_snapshots WHERE account_id=$1 AND provider='box' AND state='deleting' ORDER BY created_at LIMIT 1 FOR UPDATE").bind(&op.account).fetch_all(&mut *tx).await?;
                    if let Some(record)=rows.first(){
                        let id=text(record,"snapshotId")?;
                        if super::deletion::snapshot_in_use(&mut tx,&op.account,id).await?{return Err(ApiError::conflict("cloud_snapshot_in_use"));}
                        if record["accountId"]!=op.account||id==op.snapshot.as_deref().unwrap_or(""){return Err(ApiError::conflict("image_snapshot_identity_mismatch"));}
                        if let Some(snapshot)=provider.snapshot(id).await.map_err(provider_error)?{if snapshot.id!=id{return Err(ApiError::conflict("image_snapshot_identity_mismatch"));}provider.delete_snapshot(id).await.map_err(provider_error)?;}
                        sqlx::query("UPDATE api_cloud_snapshots SET state='deleted',record=record||$3 WHERE provider='box' AND snapshot_id=$1 AND account_id=$2 AND state='deleting'").bind(id).bind(&op.account).bind(json!({"state":"deleted","deletedAtMs":now()})).execute(&mut *tx).await?;
                    }else{op.phase="complete".into();}
                },_=>return Err(unavailable()),
            }Ok(())
        }.await;
        if let Err(error) = action {
            sqlx::query("ROLLBACK TO SAVEPOINT image_step")
                .execute(&mut *tx)
                .await?;
            if error.code == "image_provider_outcome_unknown"
                || error.code == "image_provider_unavailable"
                || error.code == "cloud_snapshot_busy"
                || error.code == "cloud_snapshot_in_use"
            {
                return Err(error);
            }
            if matches!(op.phase.as_str(), "cleanup" | "retire-images") {
                return Err(error);
            }
            op.failure = Some(error.code.into());
            op.phase = "cleanup".into();
        }
        let live:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_jobs WHERE job_id=$1 AND lease_token=$2 AND state='running' AND lease_expires_at>$3)").bind(&job.id).bind(&job.lease_token).bind(now()).fetch_one(&mut *tx).await?;
        if !live {
            return Err(ApiError::conflict("image_build_fenced"));
        }
        sqlx::query(
            "UPDATE zuse_rust_image_operations SET record=$2,updated_at=$3 WHERE build_id=$1",
        )
        .bind(&op.build)
        .bind(serde_json::to_value(&op).map_err(|_| unavailable())?)
        .bind(now())
        .execute(&mut *tx)
        .await?;
        if !matches!(op.phase.as_str(), "complete" | "retire-images") || op.failure.is_some() {
            let state = if op.failure.is_some() {
                "failed"
            } else if matches!(
                op.phase.as_str(),
                "capture" | "verify" | "cleanup" | "promote"
            ) {
                "sanitizing"
            } else {
                "building"
            };
            sqlx::query("UPDATE api_cloud_project_builds SET state=$2,provider_sandbox_id=COALESCE($3,provider_sandbox_id),last_error_code=$4,log_text=COALESCE($5,log_text),settings=settings||$6,revision=revision+1,updated_at=$7 WHERE build_id=$1").bind(&op.build).bind(state).bind(&op.machine).bind(&op.failure).bind(log.map(|s|s.chars().take(65536).collect::<String>())).bind(json!({"rustImagePhase":op.phase})).bind(now()).execute(&mut *tx).await?;
        }
        if op.phase == "complete" && op.failure.is_some() {
            for repo in &op.repositories {
                sqlx::query("UPDATE api_cloud_projects SET state='failed',last_error_code=$3,updated_at=$4 WHERE project_id=$1 AND account_id=$2 AND state='preparing' AND configuration_digest=$5").bind(text(&repo.row,"project_id")?).bind(&op.account).bind(&op.failure).bind(now()).bind(text(&repo.row,"configuration_digest")?).execute(&mut *tx).await?;
            }
        }
        tx.commit().await?;
        Ok(op.phase == "complete")
    }
}
fn validate_manifest(op: &Operation, manifest: &ImageManifest) -> Result<(), ApiError> {
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&op.plan).map_err(|_| unavailable())?)
    );
    if manifest.version != 1
        || manifest.build_id != op.build
        || manifest.template_version != op.config.template_version
        || manifest.configuration_digest != op.plan.configuration_digest
        || manifest.plan_digest != digest
        || !manifest.credentials_removed
        || manifest.runtime_rpc_protocol != "zuse-rpc-v1"
        || manifest.rust_runtime_storage_format != zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT
        || manifest.rust_session_command_version != 1
        || manifest.repositories.len() != op.repositories.len()
    {
        return Err(ApiError::conflict("image_manifest_mismatch"));
    }
    for (actual, expected) in manifest.repositories.iter().zip(&op.plan.repositories) {
        let path = op.plan.cache_root.join(format!(
            "{:x}.git",
            Sha256::digest(expected.repository_identity.as_bytes())
        ));
        if actual.project_id != expected.project_id
            || actual.repository_identity != expected.repository_identity
            || actual.default_branch != expected.default_branch
            || actual.path != path
            || !matches!(actual.source_commit.len(), 40 | 64)
            || !actual.source_commit.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(ApiError::conflict("image_manifest_mismatch"));
        }
    }
    Ok(())
}
async fn observe(
    tx: &mut Transaction<'_, Postgres>,
    op: &Operation,
    machine: &str,
) -> Result<(), ApiError> {
    sqlx::query("INSERT INTO api_cloud_runtime_observations(provider,provider_sandbox_id,observation) VALUES('boxd',$1,$2) ON CONFLICT(provider,provider_sandbox_id) DO UPDATE SET observation=EXCLUDED.observation").bind(machine).bind(json!({"accountId":op.account,"resourceKind":"build","resourceId":op.build,"provider":"boxd","providerSandboxId":machine,"observedAtMs":now()})).execute(&mut **tx).await?;
    Ok(())
}

// Reservations consume capacity but never emit charges; canonical provider reports settle costs.
async fn reserve(
    tx: &mut Transaction<'_, Postgres>,
    op: &Operation,
    at: i64,
) -> Result<(), ApiError> {
    require_billing_owner(tx, &op.account).await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,68130))")
        .bind(&op.account)
        .execute(&mut **tx)
        .await?;
    let period:Value=sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_billing_periods p WHERE account_id=$1 AND period_start<=$2 AND period_end>$2 ORDER BY period_start DESC LIMIT 1 FOR UPDATE").bind(&op.account).bind(at).fetch_optional(&mut **tx).await?.ok_or_else(||ApiError::forbidden("cloud_billing_period_missing"))?;
    let (cpu, memory) = op.resources.ok_or_else(unavailable)?;
    let start = op
        .created
        .max(period["period_start"].as_i64().ok_or_else(unavailable)?);
    let end = (at + 60_000).min(period["period_end"].as_i64().ok_or_else(unavailable)?);
    let prices:Vec<Value>=sqlx::query_scalar("SELECT to_jsonb(p) FROM api_provider_price_schedule p WHERE provider=(CASE WHEN EXISTS(SELECT 1 FROM api_provider_price_schedule WHERE provider='boxd') THEN 'boxd' ELSE 'box' END) AND effective_at<$1 ORDER BY effective_at").bind(end).fetch_all(&mut **tx).await?;
    if !prices
        .iter()
        .any(|p| p["effective_at"].as_i64().is_some_and(|t| t <= start))
    {
        return Err(ApiError::unavailable("image_provider_price_missing"));
    }
    let mut cost = 0i128;
    for (index, price) in prices.iter().enumerate() {
        let from = start.max(price["effective_at"].as_i64().ok_or_else(unavailable)?);
        let until = end.min(
            prices
                .get(index + 1)
                .and_then(|p| p["effective_at"].as_i64())
                .unwrap_or(end),
        );
        if until <= from {
            continue;
        }
        let rate = |key: &str| -> Result<i128, ApiError> {
            Ok(i128::from(
                price[key]
                    .as_i64()
                    .filter(|v| *v >= 0)
                    .ok_or_else(unavailable)?,
            ))
        };
        let nanos = rate("base_nano_usd_per_second")? * 1024
            + i128::from(cpu) * rate("cpu_nano_usd_per_second")? * 1024
            + i128::from(memory) * rate("memory_nano_usd_per_gib_second")?;
        let mut duration = until - from;
        if let Some(verify) = op.verifier_started_at {
            duration += (until - from.max(verify)).max(0);
        }
        cost += i128::from(duration) * nanos / (1_000_000 * 1024);
    }
    let cost = i64::try_from(cost)
        .ok()
        .filter(|c| *c <= 9_007_199_254_740_991)
        .ok_or_else(unavailable)?;
    sqlx::query("INSERT INTO api_cloud_billing_reservations(period_id,account_id,resource_kind,resource_id,provider,provider_cost_micros,started_at,vcpu_count,memory_mib,expires_at,updated_at) VALUES($1,$2,'build',$3,'boxd',$4,$5,$6,$7,$8,$9) ON CONFLICT(period_id,resource_kind,resource_id) DO UPDATE SET provider_cost_micros=EXCLUDED.provider_cost_micros,started_at=EXCLUDED.started_at,vcpu_count=EXCLUDED.vcpu_count,memory_mib=EXCLUDED.memory_mib,expires_at=EXCLUDED.expires_at,updated_at=EXCLUDED.updated_at").bind(text(&period,"period_id")?).bind(&op.account).bind(&op.build).bind(cost).bind(start).bind(i64::from(cpu)).bind(i64::try_from(memory).map_err(|_|unavailable())?).bind(at+120_000).bind(at).execute(&mut **tx).await?;
    crate::billing::require_capacity(tx, &op.account, at).await
}

pub(super) async fn snapshot_guard(
    tx: &mut Transaction<'_, Postgres>,
    account: &str,
) -> Result<(), ApiError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!("snapshot:{account}"))
        .execute(&mut **tx)
        .await?;
    let busy:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_snapshot_leases WHERE account_id=$1 AND expires_at>floor(extract(epoch FROM clock_timestamp())*1000)::bigint)").bind(account).fetch_one(&mut **tx).await?;
    if busy {
        return Err(ApiError::conflict("cloud_snapshot_busy"));
    }
    Ok(())
}
