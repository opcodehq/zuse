//! Account machine recipes. Writes use optimistic revisions like project
//! environments; an empty recipe means the account starts from the shared base.
use super::*;

fn digest(recipe: &MachineRecipe) -> Result<String, ApiError> {
    let bytes =
        serde_json::to_vec(recipe).map_err(|_| ApiError::bad_request("invalid_machine_recipe"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// The latest recipe's instructions, skills and files for a task computer's
/// bootstrap, or `None` when there is nothing to apply. Reads in the caller's
/// transaction and never fails it: a missing table (an API ahead of its
/// migration) or an unreadable record just means no delivery.
pub(crate) async fn delivery(
    connection: &mut sqlx::PgConnection,
    owner: &str,
) -> Result<Option<MachineDelivery>, ApiError> {
    let ready: bool =
        sqlx::query_scalar("SELECT to_regclass('cloud_machine_versions') IS NOT NULL")
            .fetch_one(&mut *connection)
            .await?;
    if !ready {
        return Ok(None);
    }
    let row: Option<(i64, Value)> = sqlx::query_as(
        "SELECT revision,recipe FROM cloud_machine_versions WHERE owner_id=$1 ORDER BY revision DESC LIMIT 1",
    )
    .bind(owner)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((revision, recipe)) = row else {
        return Ok(None);
    };
    let Ok(recipe) = serde_json::from_value::<MachineRecipe>(recipe) else {
        tracing::warn!(revision, "Machine recipe unreadable; nothing delivered");
        return Ok(None);
    };
    let delivery = recipe.delivery(revision);
    if let Some(Err(code)) = delivery.as_ref().map(MachineDelivery::validate) {
        tracing::warn!(revision, code, "Machine recipe not deliverable");
        return Ok(None);
    }
    Ok(delivery)
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
            image_state: if request.recipe.is_empty() {
                "none".into()
            } else {
                "queued".into()
            },
            image_error: None,
            recipe: request.recipe,
            created_at: now,
        })
    }
}

impl Store {
    pub async fn machine(&self, owner: &str) -> Result<MachineVersion, ApiError> {
        let row: Option<(i64, Value, String, i64)> = sqlx::query_as(
            "SELECT revision,recipe,digest,created_at FROM cloud_machine_versions WHERE owner_id=$1 ORDER BY revision DESC LIMIT 1",
        )
        .bind(owner)
        .fetch_optional(&self.pool)
        .await?;
        let Some((revision, recipe, digest, created_at)) = row else {
            return Ok(MachineVersion {
                revision: 0,
                recipe: MachineRecipe::default(),
                created_at: 0,
                image_state: "none".into(),
                image_error: None,
            });
        };
        let recipe: MachineRecipe = serde_json::from_value(recipe)
            .map_err(|_| ApiError::unavailable("machine_record_invalid"))?;
        let (image_state, image_error) = if recipe.is_empty() {
            ("none".to_owned(), None)
        } else {
            let image: Option<(String, Option<String>)> = sqlx::query_as(
                "SELECT state,error_code FROM cloud_machine_images WHERE owner_id=$1 AND recipe_digest=$2 AND state<>'retired' ORDER BY updated_at DESC LIMIT 1",
            )
            .bind(owner)
            .bind(&digest)
            .fetch_optional(&self.pool)
            .await?;
            image.unwrap_or_else(|| ("queued".to_owned(), None))
        };
        Ok(MachineVersion {
            revision,
            recipe,
            created_at,
            image_state,
            image_error,
        })
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
            .await
    }
}
