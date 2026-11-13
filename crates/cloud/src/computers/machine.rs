//! Account machine recipes. Writes use optimistic revisions like project
//! environments; an empty recipe means the account starts from the shared base.
use super::*;

fn digest(recipe: &MachineRecipe) -> Result<String, ApiError> {
    let bytes =
        serde_json::to_vec(recipe).map_err(|_| ApiError::bad_request("invalid_machine_recipe"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

impl ComputerControl {
    pub async fn save_machine_scoped(
        &self,
        access: &crate::access::Access,
        request: SaveMachine,
        now: i64,
    ) -> Result<MachineVersion, ApiError> {
        let actor = access.membership.as_ref().map(|m| serde_json::json!({"subject":access.actor.account_id,"membershipId":m.id,"organizationId":m.organization_id,"role":m.role.slug}));
        self.save_machine_as(
            &access.owner_id,
            &access.actor.account_id,
            actor,
            request,
            now,
        )
        .await
    }
    pub(crate) async fn save_machine_as(
        &self,
        owner: &str,
        actor: &str,
        actor_metadata: Option<Value>,
        request: SaveMachine,
        now: i64,
    ) -> Result<MachineVersion, ApiError> {
        if !self.accounts.contains(owner) {
            return Err(ApiError::forbidden("cloud_environment_writes_disabled"));
        }
        request.recipe.validate().map_err(ApiError::bad_request)?;
        if request.expected_revision < 0 || request.expected_revision == i64::MAX {
            return Err(ApiError::bad_request("invalid_machine_revision"));
        }
        let digest = digest(&request.recipe)?;
        let mut tx = self.pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("machine:{owner}"))
            .execute(&mut *tx)
            .await?;
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT max(revision) FROM cloud_machine_versions WHERE owner_id=$1",
        )
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
        if current.unwrap_or(0) != request.expected_revision {
            return Err(ApiError::conflict("machine_revision_changed"));
        }
        let revision = request.expected_revision + 1;
        let recipe = serde_json::to_value(&request.recipe)
            .map_err(|_| ApiError::bad_request("invalid_machine_recipe"))?;
        let previous: Option<Value> = sqlx::query_scalar(
            "SELECT recipe FROM cloud_machine_versions WHERE owner_id=$1 AND revision=$2",
        )
        .bind(owner)
        .bind(request.expected_revision)
        .fetch_optional(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO cloud_machine_versions(owner_id,revision,actor_id,actor,recipe,digest,created_at) VALUES($1,$2,$3,$4,$5,$6,$7)")
            .bind(owner).bind(revision).bind(actor).bind(actor_metadata).bind(&recipe).bind(&digest).bind(now)
            .execute(&mut *tx)
            .await?;
        // Only the setup script is in the image: an instructions-only edit
        // queues nothing. Removing the script supersedes a waiting build.
        use crate::project_caches::user_images::{image_digest, worker};
        let before = previous
            .and_then(|v| serde_json::from_value::<MachineRecipe>(v).ok())
            .and_then(|r| image_digest(&r));
        let after = image_digest(&request.recipe);
        if before != after && worker::schema_ready_tx(&mut tx).await? {
            worker::enqueue_tx(
                &mut tx,
                owner,
                "recipe",
                now + worker::SAVE_DEBOUNCE_MS,
                now,
            )
            .await?;
        }
        tx.commit().await?;
        Ok(MachineVersion {
            revision,
            image_state: if crate::project_caches::user_images::image_digest(&request.recipe)
                .is_none()
            {
                "none".into()
            } else {
                "queued".into()
            },
            image_error: None,
            image: None,
            last_good_image: None,
            recipe: request.recipe,
            created_at: now,
        })
    }
}

/// Owner-facing view of one `cloud_machine_images` row.
fn image_build(row: &Value) -> MachineImageBuild {
    let state = match row["state"].as_str() {
        Some("cleaning" | "failed") => "failed",
        Some(other) => other,
        None => "queued",
    };
    MachineImageBuild {
        state: state.into(),
        phase: (state == "building")
            .then(|| row["phase"].as_str().map(str::to_owned))
            .flatten(),
        reason: row["reason"].as_str().unwrap_or_default().into(),
        recipe_revision: row["recipe_revision"].as_i64().unwrap_or_default(),
        base_release_id: row["base_release_id"].as_str().map(str::to_owned),
        built_at: row["built_at"].as_i64(),
        error: row["error_code"].as_str().map(str::to_owned),
        log_excerpt: (state == "failed")
            .then(|| row["log_excerpt"].as_str().map(str::to_owned))
            .flatten(),
        updated_at: row["updated_at"].as_i64().unwrap_or_default(),
    }
}

impl Store {
    pub async fn machine(&self, owner: &str) -> Result<MachineVersion, ApiError> {
        let row: Option<(i64, Value, i64)> = sqlx::query_as(
            "SELECT revision,recipe,created_at FROM cloud_machine_versions WHERE owner_id=$1 ORDER BY revision DESC LIMIT 1",
        )
        .bind(owner)
        .fetch_optional(&self.pool)
        .await?;
        let Some((revision, recipe, created_at)) = row else {
            return Ok(MachineVersion {
                revision: 0,
                recipe: MachineRecipe::default(),
                created_at: 0,
                image_state: "none".into(),
                image_error: None,
                image: None,
                last_good_image: None,
            });
        };
        let recipe: MachineRecipe = serde_json::from_value(recipe)
            .map_err(|_| ApiError::unavailable("machine_record_invalid"))?;
        let mut version = MachineVersion {
            revision,
            recipe,
            created_at,
            image_state: "none".into(),
            image_error: None,
            image: None,
            last_good_image: None,
        };
        // Only the setup script is built into an image.
        let Some(current) = crate::project_caches::user_images::image_digest(&version.recipe)
        else {
            return Ok(version);
        };
        version.image_state = "queued".into();
        let mut tx = self.pool.begin().await?;
        if !crate::project_caches::user_images::worker::schema_ready_tx(&mut tx).await? {
            tx.commit().await?;
            return Ok(version);
        }
        let rows: Vec<Value> = sqlx::query_scalar(
            "(SELECT to_jsonb(i) FROM cloud_machine_images i WHERE owner_id=$1 AND state NOT IN('superseded','retiring','retired') ORDER BY created_at DESC,image_id DESC LIMIT 1) UNION ALL (SELECT to_jsonb(i) FROM cloud_machine_images i WHERE owner_id=$1 AND state='ready' ORDER BY built_at DESC,image_id DESC LIMIT 1)",
        )
        .bind(owner)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        let latest = rows
            .iter()
            .find(|r| r["state"] != "ready" || r["recipe_digest"] == current.as_str())
            .or(rows.first());
        let good = rows.iter().find(|r| r["state"] == "ready");
        if let Some(latest) = latest {
            let mut build = image_build(latest);
            // A ready image of an older script: the current one is not built yet.
            if build.state == "ready" && latest["recipe_digest"] != current.as_str() {
                build = MachineImageBuild {
                    state: "queued".into(),
                    reason: "recipe".into(),
                    recipe_revision: revision,
                    ..Default::default()
                };
            }
            version.image_state = build.state.clone();
            version.image_error = build.error.clone().filter(|_| build.state == "failed");
            version.image = Some(build);
        }
        if let Some(good) = good
            && version.image_state != "ready"
        {
            version.last_good_image = Some(image_build(good));
        }
        Ok(version)
    }
    /// "Rebuild now": a fresh build of the current setup script on the
    /// newest verified base, even if an identical image is ready.
    pub async fn rebuild_machine_scoped(
        &self,
        access: &crate::access::Access,
        now: i64,
    ) -> Result<MachineVersion, ApiError> {
        let control = self
            .computers
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_environment_writes_disabled"))?;
        if !control.accounts.contains(&access.owner_id) {
            return Err(ApiError::forbidden("cloud_environment_writes_disabled"));
        }
        let mut tx = self.pool.begin().await?;
        if !crate::project_caches::user_images::worker::schema_ready_tx(&mut tx).await? {
            return Err(ApiError::unavailable("machine_image_builder_unavailable"));
        }
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
            .bind(format!("machine:{}", access.owner_id))
            .execute(&mut *tx)
            .await?;
        if !crate::project_caches::user_images::worker::enqueue_tx(
            &mut tx,
            &access.owner_id,
            "manual",
            now,
            now,
        )
        .await?
        {
            return Err(ApiError::conflict("machine_setup_script_required"));
        }
        tx.commit().await?;
        self.machine(&access.owner_id).await
    }
    pub async fn save_machine_scoped(
        &self,
        access: &crate::access::Access,
        request: SaveMachine,
        now: i64,
    ) -> Result<MachineVersion, ApiError> {
        self.computers
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_environment_writes_disabled"))?
            .save_machine_scoped(access, request, now)
            .await?;
        self.machine(&access.owner_id).await
    }
}
