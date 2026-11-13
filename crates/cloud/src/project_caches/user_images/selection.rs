//! Which artifact a new or restored computer boots: the account's newest
//! ready user image whose base release is still verified, else the managed
//! base. The image keeps the base's pins byte-identical, so the launch keeps
//! the base's release settings and fresh-boot verification unchanged; only
//! the snapshot differs. Images are locked FOR SHARE until the computer's row
//! references them, so retirement (which waits for that lock and then checks
//! references) can never delete an image a computer was just admitted onto.
use super::*;
use serde_json::json;
use sqlx::{Postgres, Transaction};

#[derive(Clone, Debug)]
pub struct UserImageHit {
    pub image_id: String,
    pub artifact_id: String,
    pub artifact_version: Option<String>,
    pub base_release_id: String,
    pub base_artifact_id: String,
    pub recipe_digest: String,
    pub built_at: i64,
}
impl UserImageHit {
    /// Recorded as `request_config.rustMachineImage`. Retirement treats any
    /// non-deleted computer with this image ID as a reference.
    pub fn config(&self) -> Value {
        json!({"version":1,"imageId":self.image_id,"artifactId":self.artifact_id,"artifactVersion":self.artifact_version,"baseReleaseId":self.base_release_id,"recipeDigest":self.recipe_digest,"builtAt":self.built_at})
    }
}

/// The account's current image digest; `None` when the recipe has no setup
/// script, in which case every computer starts from the managed base.
pub(crate) async fn current_digest_tx(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
) -> Result<Option<String>, ApiError> {
    let recipe: Option<Value> = sqlx::query_scalar(
        "SELECT recipe FROM cloud_machine_versions WHERE owner_id=$1 ORDER BY revision DESC LIMIT 1",
    )
    .bind(owner)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(recipe
        .and_then(|r| serde_json::from_value::<MachineRecipe>(r).ok())
        .and_then(|r| image_digest(&r)))
}

/// A ready image row is usable on `base` (a locked registry row) only if
/// its frozen plan is intact and names exactly that base and its pins.
fn usable(row: &Value, base: &Value, provider: ProviderId) -> Option<UserImageHit> {
    let release = super::worker::base_from_row(base, provider)?;
    let plan: BuildPlan = serde_json::from_value(row["plan"].clone()).ok()?;
    plan.validate().ok()?;
    let artifact = row["artifact_id"].as_str().filter(|a| !a.is_empty())?;
    if row["state"] != "ready"
        || plan.image_id != row["image_id"].as_str()?
        || plan.identity.provider != provider
        || plan.identity.base_release_id != release.release_id
        || plan.identity.base_artifact_id != release.artifact_id
        || plan.identity.architecture != release.architecture
        || Some(&plan.initial_runtime_bootstrap) != release.initial_runtime_bootstrap.as_ref()
        || row["identity_key"].as_str() != plan.identity.key().ok().as_deref()
        || artifact == release.artifact_id
    {
        return None;
    }
    Some(UserImageHit {
        image_id: plan.image_id.clone(),
        artifact_id: artifact.to_owned(),
        artifact_version: row["artifact_version"].as_str().map(str::to_owned),
        base_release_id: release.release_id,
        base_artifact_id: release.artifact_id,
        recipe_digest: plan.identity.recipe_digest,
        built_at: row["built_at"].as_i64()?,
    })
}

/// For a new computer. `source` picks the managed-base build for a release
/// (agent, model and size compatibility); a newest image on an older, still
/// verified base is used only when that base can run the requested chat.
/// Returns the build to launch from and the image, if any.
pub(crate) async fn for_new_computer_tx<F>(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    provider: &str,
    build: &Value,
    source: F,
) -> Result<Option<(Value, UserImageHit)>, ApiError>
where
    F: Fn(&Value) -> Result<Value, ApiError>,
{
    let Some(provider_id) = provider_id(provider).filter(|p| PROVIDERS.contains(p)) else {
        return Ok(None);
    };
    if current_digest_tx(tx, owner).await?.is_none() {
        return Ok(None);
    }
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(i) FROM cloud_machine_images i WHERE owner_id=$1 AND provider=$2 AND state='ready' ORDER BY built_at DESC,image_id LIMIT 4 FOR SHARE")
        .bind(owner)
        .bind(provider)
        .fetch_all(&mut **tx)
        .await?;
    for row in rows {
        let Some(release) = row["base_release_id"].as_str() else {
            continue;
        };
        let Some(base) = crate::managed_bases::locked_base_record(tx, provider, release).await?
        else {
            continue;
        };
        let Some(hit) = usable(&row, &base, provider_id) else {
            continue;
        };
        if build["settings"]["managedReleaseId"] == release {
            if build["snapshot_id"] != hit.base_artifact_id.as_str() {
                continue;
            }
            return Ok(Some((build.clone(), hit)));
        }
        // An image on an older base still serves until its rebuild on the
        // newest base verifies, provided that base offers this chat.
        if let Ok(older) = source(&base) {
            if older["snapshot_id"] == hit.base_artifact_id.as_str()
                && older["settings"]["managedReleaseId"] == release
            {
                return Ok(Some((older, hit)));
            }
        }
    }
    Ok(None)
}

/// For a restore (unarchive, or "Update computer" onto `release`): the newest
/// ready image on exactly that release, else the managed base. The release
/// itself never changes here.
pub(crate) async fn for_restore_tx(
    tx: &mut Transaction<'_, Postgres>,
    owner: &str,
    provider: &str,
    release: &str,
) -> Result<Option<UserImageHit>, ApiError> {
    let Some(provider_id) = provider_id(provider).filter(|p| PROVIDERS.contains(p)) else {
        return Ok(None);
    };
    if !super::worker::schema_ready_tx(tx).await? || current_digest_tx(tx, owner).await?.is_none() {
        return Ok(None);
    }
    let Some(base) = crate::managed_bases::locked_base_record(tx, provider, release).await? else {
        return Ok(None);
    };
    let rows: Vec<Value> = sqlx::query_scalar("SELECT to_jsonb(i) FROM cloud_machine_images i WHERE owner_id=$1 AND provider=$2 AND base_release_id=$3 AND state='ready' ORDER BY built_at DESC,image_id LIMIT 4 FOR SHARE")
        .bind(owner)
        .bind(provider)
        .bind(release)
        .fetch_all(&mut **tx)
        .await?;
    Ok(rows.iter().find_map(|row| usable(row, &base, provider_id)))
}
