//! Authenticated initial creation. Existing identities, billing rows and ciphertext remain authoritative.
use super::*;
use crate::{access::Access, store::Workspace};
use zuse_protocol::cloud::CloudRuntimeMode;

pub use zuse_protocol::cloud::CloudWorkspaceCreateRequest as CreateWorkspace;
fn validate(body: &CreateWorkspace) -> Result<(), ApiError> {
    if body
        .agent_connection_id
        .as_ref()
        .is_some_and(|id| id.is_empty() || id.len() > 200)
        || body
            .environment_revision
            .is_some_and(|revision| revision < 0)
    {
        return Err(ApiError::bad_request(
            "invalid_cloud_connection_or_environment",
        ));
    }
    if body.attachments.len() > zuse_protocol::cloud_assets::MAX_ASSETS
        || (!body.attachments.is_empty()
            && body.initial_message_delivery.as_deref() == Some("mailbox-v1"))
    {
        return Err(ApiError::bad_request("invalid_launch_attachments"));
    }
    if body.idempotency_key.is_empty()
        || body.idempotency_key.len() > 512
        || body.project_id.is_empty()
        || body.project_id.len() > 128
        || body.model.is_empty()
        || body.model.len() > 512
        || body
            .first_message
            .as_ref()
            .is_some_and(|s| s.len() > 128 * 1024)
    {
        return Err(ApiError::bad_request("invalid_cloud_workspace_request"));
    }
    if !valid_git_ref(&body.base_ref)
        || body
            .branch
            .as_deref()
            .is_some_and(|s| !valid_git_ref(s) || s == "HEAD")
    {
        return Err(ApiError::bad_request("invalid_git_ref"));
    }
    if !zuse_protocol::cloud_computers::ProviderId::ALL
        .iter()
        .any(|p| p.as_str() == body.provider_id)
    {
        return Err(ApiError::bad_request("cloud_provider_unavailable"));
    }
    if body.size_id.as_deref().is_some_and(|s| {
        s.is_empty()
            || s.len() > 64
            || !s
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    }) {
        return Err(ApiError::bad_request("cloud_size_unavailable"));
    }
    let agent = if body.agent == "claude" {
        "claude-code"
    } else {
        &body.agent
    };
    let harness = zuse_protocol::HarnessId::try_from(agent.to_owned())
        .map_err(|_| ApiError::bad_request("runtime_agent_unsupported"))?;
    if harness == zuse_protocol::HarnessId::Mock {
        return Err(ApiError::bad_request("runtime_test_harness_disabled"));
    }
    if body.permissions.as_ref().is_some_and(|v| !v.is_empty())
        || matches!(
            body.runtime_mode,
            Some(CloudRuntimeMode::AutoAcceptEdits | CloudRuntimeMode::AutoAcceptEditsAndBash)
        )
    {
        return Err(ApiError::conflict("runtime_permission_import_required"));
    }
    if body.secret_bindings.as_ref().is_some_and(|refs| {
        refs.len() > 64
            || refs.iter().any(|id| {
                id.is_empty()
                    || id.len() > 200
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            })
    }) {
        return Err(ApiError::bad_request("invalid_workload_secret_binding"));
    }
    if body.local_device_id.is_some() {
        return Err(ApiError::conflict("device_bridge_runtime_update_required"));
    }
    if body.fork_source.is_some() {
        return Err(ApiError::bad_request("cloud_fork_endpoint_required"));
    }
    if body
        .initial_message_delivery
        .as_deref()
        .is_some_and(|s| !matches!(s, "mailbox-v1" | "server-admitted-v2"))
    {
        return Err(ApiError::bad_request("invalid_initial_message_delivery"));
    }
    Ok(())
}

pub fn valid_git_ref(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.starts_with(['-', '/', '.'])
        && !value.ends_with(['.', '/'])
        && !value.contains("..")
        && !value.contains("//")
        && !value.contains("@{")
        && value
            .split('/')
            .all(|part| !part.starts_with('.') && !part.ends_with(".lock"))
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._/#-".contains(&c))
}
impl RuntimeControl {
    pub fn with_creation_accounts(mut self, accounts: Vec<String>) -> Self {
        self.creation_accounts = accounts;
        self
    }
    pub fn creation_enabled(&self) -> bool {
        !self.creation_accounts.is_empty()
    }
    pub fn creation_enabled_for(&self, owner: &str) -> bool {
        self.creation_accounts
            .iter()
            .any(|account| account == owner)
    }
    #[tracing::instrument(name="cloud_runtime_request", skip_all, fields(owner=%access.owner_id, operation="create_workspace"))]
    pub async fn create_workspace(
        &self,
        access: &Access,
        body: CreateWorkspace,
        now: i64,
    ) -> Result<(Value, bool), ApiError> {
        self.create_workspace_impl(access, body, now, false).await
    }
    /// A Slack task creates a personal workspace for the linked member; Slack
    /// never creates organization runtimes.
    pub(crate) async fn create_slack_workspace(
        &self,
        access: &Access,
        body: CreateWorkspace,
        now: i64,
    ) -> Result<(Value, bool), ApiError> {
        if access.membership.is_some() || access.owner_id.starts_with("organization:") {
            return Err(ApiError::conflict("slack_organization_runtime_unavailable"));
        }
        self.create_workspace_impl(access, body, now, true).await
    }
    async fn create_workspace_impl(
        &self,
        access: &Access,
        body: CreateWorkspace,
        now: i64,
        slack: bool,
    ) -> Result<(Value, bool), ApiError> {
        if !creation_actor_allowed(access) || !self.creation_accounts.contains(&access.owner_id) {
            return Err(ApiError::forbidden("cloud_workspace_creation_disabled"));
        }
        validate(&body)?;
        // An accepted idempotent retry must return its original receipt even if
        // repository access later changed. Default admission preflights access;
        // the selected pilot gates repository access during task preparation.
        let github_authority = self
            .scope_authority
            .get()
            .and_then(|a| a.github.clone())
            .filter(|g| g.enabled(&access.owner_id));
        // The selected personal pilot admits compute locally. Repository access
        // remains gated by the fresh selected-user Git grant during preparation.
        let deferred_repository_access = self
            .fast_start_accounts
            .iter()
            .any(|owner| owner == &access.owner_id)
            && access.membership.is_none()
            && self.authority.is_some()
            && body.provider_id == "boxd"
            && body.agent == "codex"
            && body.agent_connection_id.is_some()
            && github_authority.is_some();
        let repository_authorization = if deferred_repository_access {
            None
        } else if let Some(github) = &github_authority {
            let prior:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE account_id=$1 AND idempotency_key=$2)").bind(&access.owner_id).bind(&body.idempotency_key).fetch_one(&self.pool).await?;
            if prior {
                None
            } else if let Some(error) = self
                .staging_candidate
                .as_ref()
                .filter(|pin| pin.applies(&access.owner_id, &body.provider_id))
                .and_then(|pin| pin.require_actor(&access.actor.account_id).err())
            {
                // Reject a nonselected pilot actor without a GitHub HTTP call.
                // Carry the error until locked idempotency resolution: an already
                // admitted matching request must still return its original receipt.
                Some(Err(error))
            } else {
                self.scope_admission(access).await?;
                let url:Option<String>=sqlx::query_scalar("SELECT repository_url FROM api_cloud_projects WHERE account_id=$1 AND project_id=$2 AND included=true").bind(&access.owner_id).bind(&body.project_id).fetch_optional(&self.pool).await?;
                // Carry a failed observation until AFTER the locked idempotency
                // lookup below. A concurrently admitted/lost-response request
                // must still resolve to its original task instead of a fresh error.
                Some(match url {
                    Some(url) => {
                        github
                            .authorize_repository(&access.owner_id, &access.actor.account_id, &url)
                            .await
                    }
                    None => Err(ApiError::not_found("cloud_project_not_found")),
                })
            }
        } else {
            None
        };
        if access.membership.is_some() && self.authority.is_none() {
            return Err(ApiError::conflict("workspace_runtime_update_required"));
        }
        self.scope_admission(access).await?;
        if self.authority.is_none() {
            if body.provider_id != "boxd" {
                return Err(ApiError::bad_request("cloud_provider_unavailable"));
            }
            if body.size_id.as_deref().is_some_and(|s| s != "default") {
                return Err(ApiError::bad_request("cloud_size_unavailable"));
            }
        }
        let owner = &access.owner_id;
        if self.authority.is_some() && body.agent_connection_id.is_none() {
            return Err(ApiError::conflict("agent_connection_required"));
        }
        let digest = self.master.digest_api(
            &if slack {
                format!("rust-slack-workspace-create:{owner}")
            } else if access.membership.is_some() {
                format!(
                    "rust-workspace-create:{owner}:actor:{}",
                    access.actor.account_id
                )
            } else {
                format!("rust-workspace-create:{owner}")
            },
            &serde_json::to_string(&body)
                .map_err(|_| ApiError::bad_request("invalid_cloud_workspace_request"))?,
        );
        let batched = self.authority.is_some() && self.admission_batch_available().await?;
        let mut tx = self
            .pool
            .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
            .await?;
        let facts = if batched {
            Some(
                self.read_admission_batch_tx(&mut tx, access, &body, now)
                    .await?,
            )
        } else {
            None
        };
        let prior = if let Some(facts) = &facts {
            if facts.get("deleted") == true {
                return Err(ApiError::conflict("cloud_workspace_deleted"));
            }
            facts.optional("prior")
        } else {
            // Serializes idempotency and branch admission with other Rust creates for this account.
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                .bind(format!("cloud-create:{owner}"))
                .execute(&mut *tx)
                .await?;
            let tombstones: bool =
                sqlx::query_scalar("SELECT to_regclass('zuse_rust_deletions') IS NOT NULL")
                    .fetch_one(&mut *tx)
                    .await?;
            if tombstones {
                let deleted:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM zuse_rust_deletions WHERE account_id=$1 AND idempotency_digest=$2 AND (expires_at IS NULL OR expires_at>$3))").bind(owner).bind(self.delete_idempotency(owner,&body.idempotency_key)).bind(now).fetch_one(&mut *tx).await?;
                if deleted {
                    return Err(ApiError::conflict("cloud_workspace_deleted"));
                }
            }
            let prior:Option<Value>=sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE account_id=$1 AND idempotency_key=$2").bind(owner).bind(&body.idempotency_key).fetch_optional(&mut *tx).await?;
            prior
        };
        if let Some(row) = prior {
            if row["request_config"]["rustCreateRequestDigest"] != digest
                || (access.membership.is_some()
                    && row["request_config"]["rustActorId"].as_str()
                        != Some(access.actor.account_id.as_str()))
            {
                return Err(ApiError::conflict("cloud_workspace_idempotency_conflict"));
            }
            return Ok((create_response(row), false));
        }
        let repository_authorization = repository_authorization.transpose()?;
        // Rollback may disable new version-two creation without stranding
        // existing tasks or their idempotent creation receipts.
        if body.initial_message_delivery.as_deref()
            == Some(zuse_protocol::cloud_task_commands::MODE)
            && !self.command_admission_creation
        {
            return Err(ApiError::conflict(
                "cloud_task_command_admission_unavailable",
            ));
        }
        if facts.is_none() {
            crate::images::deletion::snapshot_read_guard(&mut tx, owner, None).await?;
        }
        let candidate_pin = self
            .staging_candidate
            .as_ref()
            .filter(|p| p.applies(owner, &body.provider_id));
        if let Some(pin) = candidate_pin {
            pin.require_actor(&access.actor.account_id)?;
            if pin.expires_at <= now {
                return Err(ApiError::conflict("staging_managed_base_candidate_expired"));
            }
            // The account admission lock is already held. Count retained pilot
            // computers under a fresh snapshot; a failed first task is preserved.
            let count:i64=sqlx::query_scalar("SELECT count(*) FROM api_cloud_workspaces WHERE account_id=$1 AND state<>'deleted' AND request_config ? 'managedCandidateSha256'").bind(owner).fetch_one(&mut *tx).await?;
            if pin.computer_limit().is_some_and(|limit| count >= limit) {
                return Err(ApiError::conflict("staging_candidate_computer_limit"));
            }
        }
        let cache_pilot = crate::project_caches::pilot::current(now)?;
        if let Some(authority) = &cache_pilot {
            if authority.pilot.owner_id != owner.as_str()
                || authority.pilot.actor_id != access.actor.account_id
            {
                return Err(ApiError::forbidden("staging_project_cache_pilot_fenced"));
            }
            if !self.project_cache_control.available_tx(&mut tx).await? {
                return Err(ApiError::unavailable("project_cache_schema_not_ready"));
            }
            authority.candidate_tx(&mut tx, now).await?;
            crate::project_caches::pilot::capacity_tx(&mut tx, None).await?;
        }
        let staging_allowance = candidate_pin.is_some_and(|p| p.allow_staging_compute);
        let entitled = if let Some(facts) = &facts {
            facts.get("entitled") == true
        } else {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_entitlements WHERE account_id=$1 AND kind='cloud-workspace' AND (status IN ('active','grace') OR (status='ended' AND paid_through>$2)) AND (paid_through IS NULL OR paid_through>$2))").bind(owner).bind(now).fetch_one(&mut *tx).await?
        };
        if !entitled && !staging_allowance {
            return Err(ApiError::forbidden("cloud_entitlement_required"));
        }
        let mut connection_owner = None;
        if let Some(connection) = &body.agent_connection_id {
            let agent = if matches!(body.agent.as_str(), "claude" | "claude-code") {
                "claude"
            } else {
                body.agent.as_str()
            };
            let selected = if let Some(facts) = &facts {
                facts.get("connectionOwner").as_str().map(str::to_owned)
            } else {
                sqlx::query_scalar("SELECT owner_id FROM cloud_agent_connections WHERE owner_id IN ($1,$2) AND connection_id=$3 AND agent=$4 AND state='connected'").bind(owner).bind(&access.actor.account_id).bind(connection).bind(agent).fetch_optional(&mut *tx).await?
            };
            connection_owner =
                Some(selected.ok_or_else(|| ApiError::conflict("agent_connection_required"))?);
        }
        if !staging_allowance {
            if let Some(facts) = &facts {
                crate::billing::validate_capacity_snapshot(
                    facts.optional("billingPeriod"),
                    facts.get("billingTotals"),
                )?;
            } else {
                crate::billing::require_capacity(&mut tx, owner, now).await?;
            }
        }
        let project = if let Some(facts) = &facts {
            facts.optional("project")
        } else {
            sqlx::query_scalar("SELECT to_jsonb(p) FROM api_cloud_projects p WHERE project_id=$1 AND account_id=$2 AND included=true FOR SHARE").bind(&body.project_id).bind(owner).fetch_optional(&mut *tx).await?
        };
        let project = project.ok_or_else(|| ApiError::not_found("cloud_project_not_found"))?;
        if let (Some(github), Some(authorization)) = (&github_authority, &repository_authorization)
        {
            github
                .revalidate_repository(
                    &mut tx,
                    authorization,
                    required(&project, "repository_url")?,
                )
                .await?;
        }
        let github_enabled = self
            .scope_authority
            .get()
            .and_then(|authority| authority.github.as_ref())
            .is_some_and(|g| g.enabled(owner));
        let build: Option<Value> = if self.authority.is_some() {
            None
        } else {
            sqlx::query_scalar("SELECT to_jsonb(b) FROM api_cloud_project_builds b WHERE account_id=$1 AND provider=$2 AND state='ready' AND (settings->>'rustRuntimeCheckoutProjectId'=$3 OR settings->'rustRuntimeRepositoryLayouts' ? $3) AND NOT EXISTS(SELECT 1 FROM api_cloud_snapshots s WHERE (s.provider=b.provider OR (b.provider='boxd' AND s.provider='box')) AND s.snapshot_id=b.snapshot_id AND s.state IN ('deleting','deleted')) ORDER BY updated_at DESC,build_id DESC LIMIT 1 FOR SHARE")
            .bind(owner).bind(&body.provider_id).bind(&body.project_id).fetch_optional(&mut *tx).await?
        };
        // Legacy account images have no certified selected-connection/harness contract.
        // The authority path uses a managed release until project caches carry that
        // release identity and their own compatible preparation attestation.
        let build = match build.filter(|_| project["state"] == "ready" && self.authority.is_none())
        {
            Some(build) => build,
            None if self.authority.is_some() && body.agent_connection_id.is_some() => {
                if project["visibility"] != "public" && !github_enabled {
                    return Err(ApiError::conflict("github_runtime_credentials_required"));
                }
                if let Some(facts) = &facts {
                    let records = facts
                        .get("bases")
                        .as_array()
                        .ok_or_else(|| ApiError::unavailable("cloud_admission_read_invalid"))?;
                    if let Some(pin) = candidate_pin {
                        let row = records.first().ok_or_else(|| {
                            ApiError::conflict("staging_managed_base_candidate_unavailable")
                        })?;
                        pin.source_from_locked_record(
                            row,
                            owner,
                            &body.provider_id,
                            &body.agent,
                            &body.model,
                            body.size_id.as_deref().unwrap_or("default"),
                            owner.starts_with("organization:"),
                            now,
                        )?
                    } else {
                        crate::managed_bases::source_from_locked_records(
                            records,
                            &body.provider_id,
                            &body.agent,
                            &body.model,
                            body.size_id.as_deref().unwrap_or("default"),
                            owner.starts_with("organization:"),
                        )?
                    }
                } else {
                    crate::managed_bases::candidate::source_for(
                        &mut tx,
                        self.staging_candidate.as_deref(),
                        owner,
                        &body.provider_id,
                        &body.agent,
                        &body.model,
                        body.size_id.as_deref().unwrap_or("default"),
                        owner.starts_with("organization:"),
                        now,
                    )
                    .await?
                }
            }
            None => return Err(ApiError::conflict("cloud_project_not_ready")),
        };
        // The account's user image (built from its setup script) replaces the
        // managed base snapshot when one is ready on a still-verified base.
        let (build, user_image) = if self.authority.is_some()
            && candidate_pin.is_none()
            && build["settings"]["managedReleaseId"].is_string()
            && build["settings"]["managedCandidateSha256"].is_null()
            && self.user_image_control.available().await?
        {
            let size = body.size_id.as_deref().unwrap_or("default");
            let organization = owner.starts_with("organization:");
            match crate::project_caches::user_images::selection::for_new_computer_tx(
                &mut tx,
                owner,
                &body.provider_id,
                &build,
                |base| {
                    crate::managed_bases::source_from_locked_records(
                        std::slice::from_ref(base),
                        &body.provider_id,
                        &body.agent,
                        &body.model,
                        size,
                        organization,
                    )
                },
                now,
            )
            .await?
            {
                Some((build, hit)) => (build, Some(hit)),
                None => (build, None),
            }
        } else {
            (build, None)
        };
        let managed = build["settings"]["managedReleaseId"].is_string();
        let server_admitted = body.initial_message_delivery.as_deref()
            == Some(zuse_protocol::cloud_task_commands::MODE);
        if server_admitted
            && (!managed
                || facts.is_none()
                || build["settings"]["rustCommandAdmissionVersion"]
                    != zuse_protocol::cloud_task_commands::VERSION
                || !self.task_command_creation_available().await?)
        {
            return Err(ApiError::conflict(
                "cloud_task_command_admission_unavailable",
            ));
        }
        if access.membership.is_some() && build["settings"]["rustOrganizationExecutionVersion"] != 1
        {
            return Err(ApiError::conflict("workspace_runtime_update_required"));
        }
        let github_user_id = if github_enabled {
            if let Some(facts) = &facts {
                facts.get("githubUserId").as_i64().filter(|id| *id > 0)
            } else {
                sqlx::query_scalar::<_,Option<i64>>("SELECT github_user_id FROM zuse_rust_github_users WHERE actor_id=$1 AND state='connected' AND NOT refresh_uncertain").bind(&access.actor.account_id).fetch_optional(&mut *tx).await?.flatten().filter(|id|*id>0)
            }
        } else {
            None
        };
        let git_brokered =
            github_user_id.is_some() && build["settings"]["rustGithubCredentialsVersion"] == 1;
        // Where GitHub is configured, every computer uses the connected GitHub
        // identity for Git and PRs. A deployment without it has no identity to
        // require; its computers keep unbrokered Git.
        if github_enabled && !git_brokered {
            return Err(ApiError::conflict("github_runtime_credentials_required"));
        }

        let environment = if self.authority.is_some() || body.environment_revision.is_some() {
            let record: Option<(i64, Value)> = if let Some(facts) = &facts {
                let row = facts
                    .optional("environment")
                    .map(|v| {
                        v["revision"]
                            .as_i64()
                            .map(|revision| (revision, v["recipe"].clone()))
                            .ok_or_else(|| ApiError::unavailable("cloud_admission_read_invalid"))
                    })
                    .transpose()?;
                if body
                    .environment_revision
                    .is_some_and(|revision| revision > 0)
                    && row.is_none()
                {
                    return Err(ApiError::conflict("environment_revision_not_found"));
                }
                row
            } else if let Some(revision) = body.environment_revision {
                if revision == 0 {
                    None
                } else {
                    let row = sqlx::query_as("SELECT revision,recipe FROM cloud_environment_versions WHERE owner_id=$1 AND project_id=$2 AND revision=$3")
                        .bind(owner).bind(&body.project_id).bind(revision).fetch_optional(&mut *tx).await?;
                    if row.is_none() {
                        return Err(ApiError::conflict("environment_revision_not_found"));
                    }
                    row
                }
            } else {
                sqlx::query_as("SELECT revision,recipe FROM cloud_environment_versions WHERE owner_id=$1 AND project_id=$2 ORDER BY revision DESC LIMIT 1")
                    .bind(owner).bind(&body.project_id).fetch_optional(&mut *tx).await?
            };
            if build["settings"]["rustEnvironmentRecipeVersion"] != 1 {
                return Err(ApiError::conflict("environment_runtime_update_required"));
            }
            let (revision, recipe) = record.unwrap_or((
                0,
                serde_json::to_value(zuse_protocol::cloud_computers::EnvironmentRecipe::default())
                    .expect("default recipe serializes"),
            ));
            let parsed: zuse_protocol::cloud_computers::EnvironmentRecipe =
                serde_json::from_value(recipe.clone())
                    .map_err(|_| ApiError::conflict("environment_recipe_invalid"))?;
            parsed.validate().map_err(ApiError::bad_request)?;
            Some((revision, recipe))
        } else {
            None
        };
        let mut workload_bindings = std::collections::BTreeSet::new();
        for id in body.secret_bindings.as_deref().unwrap_or_default() {
            workload_bindings.insert(id.clone());
        }
        for binding in project["secret_bindings"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(
                environment
                    .as_ref()
                    .and_then(|(_, recipe)| recipe["secretBindings"].as_array())
                    .into_iter()
                    .flatten(),
            )
        {
            workload_bindings.insert(
                binding
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("invalid_workload_secret_binding"))?
                    .to_owned(),
            );
        }
        let workload_bindings = workload_bindings.into_iter().collect::<Vec<_>>();
        if !workload_bindings.is_empty() {
            if !managed || build["settings"]["rustWorkloadSecretsVersion"] != 1 {
                return Err(ApiError::conflict(
                    "workload_secret_runtime_update_required",
                ));
            }
            if let Some(facts) = &facts {
                self.validate_workload_binding_rows(
                    owner,
                    &body.project_id,
                    &workload_bindings,
                    facts
                        .get("secretRows")
                        .as_array()
                        .ok_or_else(|| ApiError::unavailable("cloud_admission_read_invalid"))?,
                )?;
            } else {
                self.validate_workload_bindings_tx(
                    &mut tx,
                    owner,
                    &body.project_id,
                    &workload_bindings,
                )
                .await?;
            }
        }
        if slack && build["settings"]["rustSlackVersion"] != 1 {
            return Err(ApiError::conflict("slack_runtime_update_required"));
        }
        if (body.reasoning.is_some() || !body.model_options.is_empty())
            && build["settings"]["rustLaunchModelConfigVersion"] != 1
        {
            return Err(ApiError::conflict(
                "cloud_model_configuration_runtime_update_required",
            ));
        }
        let layout = build["settings"]["rustRuntimeRepositoryLayouts"].get(&body.project_id);
        if let Some(layout) = layout {
            let cache = layout["cachePath"]
                .as_str()
                .ok_or_else(|| ApiError::conflict("cloud_project_runtime_update_required"))?;
            let commit = layout["sourceCommit"].as_str().unwrap_or("");
            let expected = format!(
                "/home/repos/{:x}.git",
                sha2::Sha256::digest(
                    project["repository_identity"]
                        .as_str()
                        .unwrap_or("")
                        .as_bytes()
                )
            );
            if cache != expected
                || !matches!(commit.len(), 40 | 64)
                || !commit.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(ApiError::conflict("cloud_project_runtime_update_required"));
            }
        }
        // Accept only attested Rust layouts, including legacy single-project images.
        if build["settings"]["runtimeRpcProtocol"] != zuse_protocol::cloud_gateway::RUST_RPC
            || build["settings"]["rustRuntimeStorageFormat"]
                != zuse_protocol::cloud::RUST_RUNTIME_STORAGE_FORMAT
            || build["settings"]["rustSessionCommandVersion"] != 1
            || (!managed
                && layout.is_none()
                && build["settings"]["rustRuntimeCheckoutProjectId"] != body.project_id)
            || build["settings"]["providerConnectionId"].is_string()
        {
            return Err(ApiError::conflict("cloud_project_runtime_update_required"));
        }
        let brokered = build["settings"]["providerAuthDeliveryVersion"] == 1;
        let codex_subscription =
            build["settings"]["providers"]
                .as_array()
                .is_some_and(|providers| {
                    providers
                        .iter()
                        .any(|p| p["providerId"] == "codex" && p["method"] == "subscription")
                });
        let codex_brokered =
            build["settings"]["codexAuthDeliveryVersion"] == 1 && codex_subscription;
        if brokered && body.agent == "codex" && codex_subscription && !codex_brokered {
            return Err(ApiError::conflict("runtime_provider_auth_adapter_required"));
        }
        if (brokered || codex_brokered)
            && body.agent_connection_id.is_none()
            && (self.grant_issuer.is_none()
                || !matches!(body.agent.as_str(), "claude" | "claude-code" | "codex"))
        {
            return Err(ApiError::conflict("runtime_provider_auth_adapter_required"));
        }
        let snapshot = build["snapshot_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ApiError::conflict("cloud_project_not_ready"))?;
        // A cache is optional and usable only after its separate runtime/provider
        // certification. Candidate or mutable-ref requests retain managed-base boot.
        let project_cache = if managed
            && workload_bindings.is_empty()
            && project["visibility"] == "public"
            && build["settings"]["managedCandidateSha256"].is_null()
            && matches!(body.base_ref.len(), 40 | 64)
            && body
                .base_ref
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            && self.project_cache_control.available_tx(&mut tx).await?
        {
            if let (Some((_, recipe)), Some(release), Some(repository)) = (
                environment.as_ref(),
                build["settings"]["managedReleaseId"].as_str(),
                project["repository_identity"].as_str(),
            ) {
                crate::project_caches::selection::for_new_task_tx(
                    &mut tx,
                    owner,
                    &body.project_id,
                    repository,
                    &body.provider_id,
                    release,
                    snapshot,
                    &body.base_ref,
                    recipe,
                )
                .await?
            } else {
                None
            }
        } else {
            None
        };
        // A project cache is built from the managed base and wins over it.
        let user_image = user_image.filter(|_| project_cache.is_none());
        let selected_snapshot = project_cache
            .as_ref()
            .map(|hit| hit.artifact_id.as_str())
            .or(user_image.as_ref().map(|hit| hit.artifact_id.as_str()))
            .unwrap_or(snapshot);
        let staging: bool = if facts.is_some() {
            true
        } else {
            sqlx::query_scalar("SELECT to_regclass('zuse_rust_launch_staging') IS NOT NULL")
                .fetch_one(&mut *tx)
                .await?
        };
        let staged: Option<(String, i64)> = if let Some(facts) = &facts {
            facts
                .optional("staging")
                .map(|v| {
                    Ok::<_, ApiError>((
                        v["workspace_id"]
                            .as_str()
                            .ok_or_else(|| ApiError::unavailable("cloud_admission_read_invalid"))?
                            .to_owned(),
                        v["expires_at"]
                            .as_i64()
                            .ok_or_else(|| ApiError::unavailable("cloud_admission_read_invalid"))?,
                    ))
                })
                .transpose()?
        } else if staging {
            sqlx::query_as("SELECT workspace_id,expires_at FROM zuse_rust_launch_staging WHERE account_id=$1 AND idempotency_digest=$2 FOR UPDATE").bind(owner).bind(self.launch_idempotency(access,&body.idempotency_key)).fetch_optional(&mut *tx).await?
        } else {
            None
        };
        if staged.as_ref().is_some_and(|(_, expiry)| *expiry <= now) {
            return Err(ApiError::conflict("cloud_launch_upload_expired"));
        }
        let id = staged.map(|(id, _)| id).unwrap_or_else(|| {
            facts
                .as_ref()
                .and_then(|f| f.get("workspaceId").as_str())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("workspace_{}", uuid::Uuid::new_v4().simple()))
        });
        if !body.attachments.is_empty() {
            if !staging {
                return Err(ApiError::unavailable("cloud_launch_asset_schema_not_ready"));
            }
            if build["settings"]["rustLaunchAssetsVersion"] != 1 {
                return Err(ApiError::conflict(
                    "cloud_launch_attachments_runtime_update_required",
                ));
            }
            if let Some(facts) = &facts {
                self.verify_launch_asset_rows(
                    owner,
                    &id,
                    &body.attachments,
                    facts
                        .get("assetRows")
                        .as_array()
                        .ok_or_else(|| ApiError::unavailable("cloud_admission_read_invalid"))?,
                )?;
            } else {
                self.verify_launch_assets(&mut tx, owner, &id, &body.attachments)
                    .await?;
            }
        }
        let chat = format!("chat_{}", uuid::Uuid::new_v4());
        let session = format!("s_{}", uuid::Uuid::new_v4());
        let branch = body
            .branch
            .clone()
            .unwrap_or_else(|| format!("cloud-{}", &id[10..22]));
        let in_use = if let Some(facts) = &facts {
            facts.get("branchInUse") == true
        } else {
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_cloud_workspaces WHERE project_id=$1 AND branch=$2 AND state<>'deleted')").bind(&body.project_id).bind(&branch).fetch_one(&mut *tx).await?
        };
        if in_use {
            return Err(ApiError::conflict("cloud_branch_in_use"));
        }
        let command = format!("launch:{id}");
        let turn = format!("turn:{id}");
        let title_text = body
            .first_message
            .as_deref()
            .map(zuse_protocol::attachment_mentions::attachment_mention_prompt);
        let line = title_text
            .as_deref()
            .unwrap_or(&branch)
            .lines()
            .next()
            .unwrap_or(&branch)
            .trim();
        let title = if line.is_empty() {
            branch.clone()
        } else if line.chars().count() > 80 {
            format!("{}…", line.chars().take(77).collect::<String>())
        } else {
            line.to_owned()
        };
        let mut intent = json!({"commandId":command,"turnId":turn,"title":title,"agent":body.agent,"model":body.model,"permissions":body.permissions.clone().unwrap_or_default(),"runtimeMode":body.runtime_mode.unwrap_or(CloudRuntimeMode::ApprovalRequired)});
        if let Some(reasoning) = body.reasoning {
            intent["reasoning"] = json!(reasoning);
        }
        if !body.model_options.is_empty() {
            intent["modelOptions"] = json!(body.model_options);
        }
        if !body.attachments.is_empty() {
            intent["attachments"] = json!(body.attachments);
        }
        let mailbox =
            server_admitted || body.initial_message_delivery.as_deref() == Some("mailbox-v1");
        let admitted_message = body.first_message.clone();
        if !mailbox && let Some(message) = body.first_message {
            intent["firstMessage"] = json!(message);
        }
        let ciphertext = self
            .master
            .seal(
                format!("zuse-cloud-workspace-launch-intent-v1\n{owner}\n{id}").as_bytes(),
                intent.to_string().as_bytes(),
            )
            .map_err(|_| ApiError::unavailable("cloud_workspace_launch_intent_unavailable"))?;
        let (_, wrapped) = self
            .master
            .create_transcript_key(owner, &id)
            .map_err(|_| ApiError::unavailable("cloud_transcript_key_unavailable"))?;
        let mut config = json!({"rustActorId":access.actor.account_id,"runtimeGeneration":1,"gatewayEpoch":1,"rustCreateRequestDigest":digest,"rustNativeTranscriptEnabled":self.transcript_objects.is_some(),"rustCommandMailboxEnabled":mailbox,"runtimeRpcProtocol":zuse_protocol::cloud_gateway::RUST_RPC,"title":title,"agent":body.agent,"initialTurnId":turn,"providerAuthMode":if brokered{"broker-v1"}else{"legacy-image"},"runtimeMode":intent["runtimeMode"],"permissions":intent["permissions"],"repositoryCache":"account-image","rustRepositoryCheckout":{"branch":branch,"baseRef":body.base_ref},"startupTimings":{"requestedAt":now}});
        if managed
            && brokered
            && self
                .fast_start_accounts
                .iter()
                .any(|account| account == owner)
        {
            config["rustFastStartVersion"] = json!(1);
        }
        if let Some(authority) = &cache_pilot {
            if authority.pilot.project_id == body.project_id && body.provider_id == "boxd" {
                config["stagingProjectCachePilotSha256"] = json!(authority.pilot.fingerprint()?);
            }
        }
        if server_admitted {
            config["commandDeliveryMode"] = json!(zuse_protocol::cloud_task_commands::MODE);
            config["rustCommandAdmissionVersion"] =
                json!(zuse_protocol::cloud_task_commands::VERSION);
        }
        if deferred_repository_access {
            config["rustRepositoryAuthorizationMode"] = json!("preparation-v1");
        }
        if git_brokered {
            config["gitAuthMode"] = json!("broker-v1");
            config["rustGithubUserId"] = json!(github_user_id);
        }
        if let Some(owner) = connection_owner {
            config["agentConnectionOwnerId"] = json!(owner);
        }
        if let Some(member) = &access.membership {
            config["rustOrganizationExecutionVersion"] = json!(1);
            config["sharingPolicy"] = json!({"audience":"private","permission":"view","creatorSubject":access.actor.account_id,"creatorMembershipId":member.id,"grants":[],"revision":1});
        }
        if managed {
            if build["settings"]["rustGithubCliVersion"] == 1 {
                config["rustGithubCliVersion"] = json!(1);
            }
            if build["settings"]["rustRecreationVersion"] == 1 {
                config["rustRecreationVersion"] = json!(1);
            }
            if build["settings"]["rustRuntimeRepairVersion"] == 1 {
                config["rustRuntimeRepairVersion"] = json!(1);
            }
            if build["settings"]["rustRuntimeSupervisorVersion"] == 1 {
                config["rustRuntimeSupervisorVersion"] = json!(1);
            }
            if build["settings"]["rustToolOutputVersion"] == 1 {
                config["rustToolOutputVersion"] = json!(1);
            }
            if build["settings"]["rustMemorySleepVersion"] == 1 {
                config["rustMemorySleepVersion"] = json!(1);
            }
            config["rustProviderControllerVersion"] = json!(1);
            config["computerSizeId"] = json!(body.size_id.as_deref().unwrap_or("default"));
            config["rustPreparationVersion"] = json!(1);
            config["managedReleaseId"] = build["settings"]["managedReleaseId"].clone();
            for key in [
                "providerArtifactBinding",
                "initialRuntimeBootstrap",
                "managedCandidateSha256",
                "managedCandidateArtifactId",
                "managedBaseReadiness",
            ] {
                if !build["settings"][key].is_null() {
                    config[key] = build["settings"][key].clone();
                }
            }
            config["repositoryCache"] = json!("none");
            config["rustRepositoryCheckout"]["repositoryUrl"] = project["repository_url"].clone();
            config["rustRepositoryCheckout"]["fetchRemote"] = json!(true);
        }
        if let Some(hit) = &project_cache {
            config["rustProjectCache"] = hit.config();
            config["rustProjectCache"]["baseArtifactBinding"] =
                build["settings"]["providerArtifactBinding"].clone();
            config["repositoryCache"] = json!("project-environment-v1");
            config["rustRepositoryCheckout"]["sourceCommit"] = json!(body.base_ref);
            config["rustRepositoryCheckout"]["cachePath"] = json!(format!(
                "/opt/zuse/project-caches/{}/repository.git",
                hit.cache_key
            ));
            config["rustRepositoryCheckout"]["fetchRemote"] = json!(false);
            // Provider-specific bindings belong to the selected artifact itself.
            config
                .as_object_mut()
                .expect("config object")
                .remove("providerArtifactBinding");
            if let Some(binding) = &hit.artifact_binding {
                config["providerArtifactBinding"] = json!(binding);
            }
        }
        if let Some(hit) = &user_image {
            config["rustMachineImage"] = hit.config();
        }
        if let Some(connection) = &body.agent_connection_id {
            if !brokered {
                return Err(ApiError::conflict("runtime_provider_auth_adapter_required"));
            }
            config["agentConnectionId"] = json!(connection);
            if body.agent == "codex" {
                config["codexAuthMode"] = json!("broker-v1");
            }
        }
        if !workload_bindings.is_empty() {
            config["rustWorkloadSecretsVersion"] = json!(1);
            config["workloadSecretBindings"] = json!(workload_bindings);
        }
        if let Some((revision, recipe)) = environment {
            config["environmentRevision"] = json!(revision);
            config["environmentRecipe"] = recipe;
        }
        if codex_brokered {
            config["codexAuthMode"] = json!("broker-v1");
        }
        if let Some(layout) = layout {
            config["rustRepositoryCheckout"]["cachePath"] = layout["cachePath"].clone();
            config["rustRepositoryCheckout"]["repositoryUrl"] = project["repository_url"].clone();
            config["rustRepositoryCheckout"]["sourceCommit"] = layout["sourceCommit"].clone();
            if project["visibility"] == "public"
                && build["settings"]["rustRepositoryFetchVersion"] == 1
            {
                config["rustRepositoryCheckout"]["fetchRemote"] = json!(true);
            }
        }
        if let Some(facts) = &facts {
            if facts
                .get("unavailableSnapshots")
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id == snapshot))
            {
                return Err(ApiError::conflict("cloud_snapshot_unavailable"));
            }
            let mut admission = intent.clone();
            admission["firstMessage"] = json!(admitted_message);
            let mut plan = self.fresh_admission_plan(
                super::admission_batch::FreshAdmissionWrite {
                    workspace_id: id.clone(),
                    owner_id: owner.clone(),
                    project_id: body.project_id.clone(),
                    build_id: build["build_id"].as_str().map(str::to_owned),
                    managed_release_id: config["managedReleaseId"].as_str().map(str::to_owned),
                    provider: body.provider_id.clone(),
                    chat_id: chat,
                    session_id: session,
                    branch,
                    base_ref: body.base_ref.clone(),
                    wrapped_transcript_key: wrapped,
                    idempotency_key: body.idempotency_key.clone(),
                    request_config: config,
                    command_id: command,
                    turn_id: turn,
                    launch_ciphertext: ciphertext,
                    initial_task_command: None,
                    admission_ciphertext: String::new(),
                    launch_expires_at: now + 86_400_000,
                    operation_id: String::new(),
                    snapshot_id: selected_snapshot.to_owned(),
                    machine_name: String::new(),
                    storage_incarnation_id: String::new(),
                    sealed_boot_token: String::new(),
                    boot_token_hash: String::new(),
                    boot_expires_at: 0,
                    job_id: String::new(),
                    now,
                },
                &admission,
            )?;
            if server_admitted {
                let release_id = plan.request_config["managedReleaseId"]
                    .as_str()
                    .ok_or_else(|| ApiError::conflict("cloud_task_managed_policy_required"))?;
                let record = facts
                    .get("bases")
                    .as_array()
                    .and_then(|rows| rows.iter().find(|row| row["release_id"] == release_id))
                    .ok_or_else(|| ApiError::conflict("cloud_task_managed_policy_required"))?;
                let release = if plan.request_config["managedCandidateSha256"].is_string() {
                    &record["candidate_manifest"]
                } else {
                    &record["runtime_manifest"]
                };
                let initial_row = json!({"workspace_id":plan.workspace_id,"account_id":plan.owner_id,"chat_id":plan.chat_id,"initial_session_id":plan.session_id,"wrapped_transcript_key":plan.wrapped_transcript_key,"request_config":plan.request_config,"state":"provisioning","desired_state":"ready","runtime_state":"offline"});
                plan.initial_task_command = Some(self.prepare_initial_task_command(
                    &initial_row,
                    access,
                    &admission,
                    release,
                    now,
                )?);
            }
            let row = self.write_fresh_admission_batch_tx(&mut tx, &plan).await?;
            if server_admitted && !body.attachments.is_empty() {
                // The first message's files are bound to its launch command.
                self.hold_assets_tx(
                    &mut tx,
                    &plan.workspace_id,
                    super::assets::Holder::Command {
                        session: &plan.session_id,
                        command: &plan.command_id,
                    },
                    &body.attachments,
                    now,
                )
                .await?;
            }
            tx.commit().await?;
            super::lifecycle::wakeup::send();
            return Ok((create_response(row), true));
        }
        // The legacy path remains usable before the additive managed-base migration.
        let (source_column, source_value) = if managed {
            (",managed_release_id", ",$12->>'managedReleaseId'")
        } else {
            ("", "")
        };
        let statement = format!(
            "INSERT INTO api_cloud_workspaces(workspace_id,account_id,project_id,build_id,provider,runtime_state,chat_id,initial_session_id,branch,base_ref,state,desired_state,status_code,wrapped_transcript_key,idempotency_key,request_config,next_action_at,revision,created_at,updated_at,last_activity_at{source_column}) VALUES($1,$2,$3,$4,$5,'offline',$6,$7,$8,$9,'queued','ready','provisioning-queued',$10,$11,$12,$13,0,$13,$13,$13{source_value})"
        );
        sqlx::query(&statement)
            .bind(&id)
            .bind(owner)
            .bind(&body.project_id)
            .bind(build["build_id"].as_str())
            .bind(&body.provider_id)
            .bind(&chat)
            .bind(&session)
            .bind(&branch)
            .bind(&body.base_ref)
            .bind(wrapped)
            .bind(&body.idempotency_key)
            .bind(config)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO api_cloud_workspace_launch_intents(workspace_id,account_id,chat_id,session_id,turn_id,command_id,ciphertext,expires_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(&id).bind(owner).bind(&chat).bind(&session).bind(&turn).bind(&command).bind(ciphertext).bind(now+86_400_000).bind(now).execute(&mut *tx).await?;
        let mut admission = intent.clone();
        admission["firstMessage"] = json!(admitted_message);
        self.save_task_admission_tx(&mut tx, owner, &id, &session, &admission, now)
            .await?;
        sqlx::query("INSERT INTO zuse_rust_control_owners(resource_kind,resource_id,backend,generation,updated_at) VALUES('workspace',$1,'rust',1,$2)").bind(&id).bind(now).execute(&mut *tx).await?;
        self.queue_allocation_tx(&mut tx, &id, selected_snapshot, now)
            .await?;
        let row: Value = sqlx::query_scalar(
            "SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1",
        )
        .bind(&id)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        super::lifecycle::wakeup::send();
        Ok((create_response(row), true))
    }
}
fn create_response(row: Value) -> Value {
    let mailbox = row["request_config"]["rustCommandMailboxEnabled"] == true;
    let v2 =
        row["request_config"]["commandDeliveryMode"] == zuse_protocol::cloud_task_commands::MODE;
    let mut response = json!({"workspace":Workspace{row}.public()});
    response["chatId"] = response["workspace"]["chatId"].clone();
    response["initialSessionId"] = response["workspace"]["initialSessionId"].clone();
    if v2 {
        response["initialMessageDelivery"] = json!(zuse_protocol::cloud_task_commands::MODE);
        response["workspace"]["taskCommandCanEdit"] = json!(true);
    } else if mailbox {
        response["initialMessageDelivery"] = json!("mailbox-v1");
    }
    response
}

/// Task creation never enrolls a member or changes seats. Scope authorization has
/// already checked the fresh membership; pin the same actor on the task.
pub(super) fn creation_actor_allowed(access: &Access) -> bool {
    match (&access.scope, &access.membership) {
        (crate::access::Scope::Personal, None) => access.owner_id == access.actor.account_id,
        (crate::access::Scope::Organization { organization_id }, Some(member)) => {
            access.owner_id == format!("organization:{organization_id}")
                && member.organization_id == *organization_id
                && member.user_id == access.actor.account_id
                && member.status == "active"
                && matches!(member.role.slug.as_str(), "admin" | "member")
        }
        _ => false,
    }
}
