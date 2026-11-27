//! Setup chats: an agent proposes a new machine image configuration and the
//! owner applies it. A proposal only records what the owner reviews; applying
//! it goes through the same writers as the settings page (repositories, the
//! recipe, "Rebuild now"), so the image, its history and its safety checks
//! are the normal ones: the previous image keeps serving until the new one
//! verifies, and builds strip credentials before capture.
use super::*;
use zuse_protocol::machine_setup::{
    self as setup, ImageConfig, ImageRepository, Proposal, ProposalView,
};

/// The account's own delivered instructions with the setup chat's on top.
/// Kept within the delivery limits: the account's part is cut, never ours.
pub(crate) fn with_setup_instructions(delivery: Option<MachineDelivery>) -> MachineDelivery {
    const LIMIT: usize = 64 * 1024;
    let mut delivery = delivery.unwrap_or(MachineDelivery {
        revision: 0,
        instructions: String::new(),
        skills: vec![],
        files: vec![],
    });
    let own = delivery.instructions.trim_end();
    let room = LIMIT.saturating_sub(setup::INSTRUCTIONS.len() + 2);
    let mut cut = own.len().min(room);
    while !own.is_char_boundary(cut) {
        cut -= 1;
    }
    delivery.instructions = if own.is_empty() {
        setup::INSTRUCTIONS.to_owned()
    } else {
        format!("{}\n\n{}", &own[..cut], setup::INSTRUCTIONS)
    };
    delivery
}

/// One included repository of the account.
struct Repository {
    project_id: String,
    identity: String,
    default_branch: String,
}

async fn repositories(
    connection: &mut sqlx::PgConnection,
    owner: &str,
) -> Result<Vec<Repository>, ApiError> {
    let present: bool = sqlx::query_scalar("SELECT to_regclass('api_cloud_projects') IS NOT NULL")
        .fetch_one(&mut *connection)
        .await?;
    if !present {
        return Ok(vec![]);
    }
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT project_id,repository_identity,default_branch FROM api_cloud_projects WHERE account_id=$1 AND included=true ORDER BY created_at,project_id",
    )
    .bind(owner)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(project_id, identity, default_branch)| Repository {
            project_id,
            identity: identity.to_lowercase(),
            default_branch,
        })
        .collect())
}

fn proposal(row: &Value) -> Result<Proposal, ApiError> {
    let invalid = || ApiError::unavailable("machine_setup_proposal_invalid");
    let text = |key: &str| row[key].as_str().map(str::to_owned).ok_or_else(invalid);
    let list = |key: &str| -> Result<Vec<String>, ApiError> {
        serde_json::from_value(row[key].clone()).map_err(|_| invalid())
    };
    Ok(Proposal {
        proposal_id: text("proposal_id")?,
        chat_id: text("chat_id")?,
        base_revision: row["base_revision"].as_i64().ok_or_else(invalid)?,
        current_setup_script: text("current_setup_script")?,
        setup_script: text("setup_script")?,
        current_repositories: list("current_repositories")?,
        repositories: list("repositories")?,
        notes: row["notes"].as_str().unwrap_or_default().to_owned(),
        state: text("state")?,
        created_at: row["created_at"].as_i64().ok_or_else(invalid)?,
        decided_at: row["decided_at"].as_i64(),
        applied_revision: row["applied_revision"].as_i64(),
    })
}

impl Store {
    /// What a setup chat's agent reads: the account's current setup script,
    /// its repositories and the image state.
    pub async fn machine_setup_config(&self, owner: &str) -> Result<ImageConfig, ApiError> {
        let machine = self.machine(owner).await?;
        let mut connection = self.pool.acquire().await?;
        let repositories = repositories(&mut connection, owner).await?;
        let baked = machine
            .image
            .iter()
            .chain(machine.last_good_image.iter())
            .flat_map(|image| image.repositories.iter())
            .collect::<Vec<_>>();
        Ok(ImageConfig {
            revision: machine.revision,
            setup_script: machine.recipe.setup_script.clone(),
            repositories: repositories
                .into_iter()
                .map(|r| ImageRepository {
                    image_state: baked
                        .iter()
                        .find(|b| b.repository_identity.eq_ignore_ascii_case(&r.identity))
                        .map(|b| b.state.clone()),
                    identity: r.identity,
                    default_branch: r.default_branch,
                })
                .collect(),
            image_state: machine.image_state,
        })
    }

    /// Store the agent's proposal; an older undecided one of the same chat
    /// is superseded. `pool` is a write pool (the store's may be read-only).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn machine_setup_propose(
        &self,
        pool: &PgPool,
        owner: &str,
        workspace: &str,
        chat: &str,
        setup_script: String,
        repositories: Vec<String>,
        notes: Option<String>,
        now: i64,
    ) -> Result<Proposal, ApiError> {
        let repositories = setup::validate_proposal(&setup_script, &repositories, notes.as_deref())
            .map_err(ApiError::bad_request)?;
        let current = self.machine_setup_config(owner).await?;
        let id = format!("msp_{}", uuid::Uuid::new_v4().simple());
        let current_repositories: Vec<String> = current
            .repositories
            .iter()
            .map(|r| r.identity.clone())
            .collect();
        let mut tx = pool.begin().await?;
        sqlx::query("UPDATE cloud_machine_setup_proposals SET state='superseded',decided_at=$3 WHERE owner_id=$1 AND workspace_id=$2 AND state='proposed'")
            .bind(owner)
            .bind(workspace)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let row: Value = sqlx::query_scalar("INSERT INTO cloud_machine_setup_proposals(proposal_id,owner_id,workspace_id,chat_id,base_revision,current_setup_script,setup_script,current_repositories,repositories,notes,state,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'proposed',$11) RETURNING to_jsonb(cloud_machine_setup_proposals.*)")
            .bind(&id)
            .bind(owner)
            .bind(workspace)
            .bind(chat)
            .bind(current.revision)
            .bind(&current.setup_script)
            .bind(&setup_script)
            .bind(serde_json::json!(current_repositories))
            .bind(serde_json::json!(repositories))
            .bind(notes.unwrap_or_default().trim())
            .bind(now)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        proposal(&row)
    }

    async fn proposal_row(&self, owner: &str, id: &str) -> Result<(Value, Proposal), ApiError> {
        if !setup::proposal_id_valid(id) {
            return Err(ApiError::not_found("machine_setup_proposal_not_found"));
        }
        let row: Option<Value> = sqlx::query_scalar(
            "SELECT to_jsonb(p) FROM cloud_machine_setup_proposals p WHERE proposal_id=$1 AND owner_id=$2",
        )
        .bind(id)
        .bind(owner)
        .fetch_optional(&self.pool)
        .await?;
        let row = row.ok_or_else(|| ApiError::not_found("machine_setup_proposal_not_found"))?;
        let parsed = proposal(&row)?;
        Ok((row, parsed))
    }

    /// A proposal with the account's machine, for the chat card.
    pub async fn machine_setup_proposal(
        &self,
        owner: &str,
        id: &str,
    ) -> Result<ProposalView, ApiError> {
        let (_, proposal) = self.proposal_row(owner, id).await?;
        Ok(ProposalView {
            proposal,
            machine: self.machine(owner).await?,
        })
    }

    fn write_pool(&self) -> Result<PgPool, ApiError> {
        Ok(self
            .computers
            .as_ref()
            .ok_or_else(|| ApiError::unavailable("cloud_environment_writes_disabled"))?
            .pool
            .clone())
    }

    /// Mark an undecided proposal; `false` when it was already decided.
    async fn decide(
        &self,
        owner: &str,
        id: &str,
        state: &str,
        revision: Option<i64>,
        now: i64,
    ) -> Result<bool, ApiError> {
        let updated = sqlx::query("UPDATE cloud_machine_setup_proposals SET state=$3,applied_revision=$4,decided_at=$5 WHERE proposal_id=$1 AND owner_id=$2 AND state='proposed'")
            .bind(id)
            .bind(owner)
            .bind(state)
            .bind(revision)
            .bind(now)
            .execute(&self.write_pool()?)
            .await?;
        Ok(updated.rows_affected() == 1)
    }

    /// The workspace that made proposal `id`, once it is decided.
    pub(crate) async fn machine_setup_workspace(
        &self,
        owner: &str,
        id: &str,
    ) -> Result<String, ApiError> {
        let (row, _) = self.proposal_row(owner, id).await?;
        row["workspace_id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| ApiError::unavailable("machine_setup_proposal_invalid"))
    }

    pub async fn discard_machine_setup(
        &self,
        access: &crate::access::Access,
        id: &str,
        now: i64,
    ) -> Result<ProposalView, ApiError> {
        let (_, proposal) = self.proposal_row(&access.owner_id, id).await?;
        if proposal.state == "proposed" {
            self.decide(&access.owner_id, id, "discarded", None, now)
                .await?;
        } else if proposal.state == "applied" {
            return Err(ApiError::conflict("machine_setup_proposal_applied"));
        }
        self.machine_setup_proposal(&access.owner_id, id).await
    }

    /// Apply proposal `id`: connect and remove repositories, save the setup
    /// script and queue the rebuild. New repositories must be in the user's
    /// GitHub catalog, as with Add repository. Applying twice is a no-op.
    pub async fn apply_machine_setup(
        &self,
        access: &crate::access::Access,
        id: &str,
        now: i64,
    ) -> Result<ProposalView, ApiError> {
        let owner = access.owner_id.as_str();
        let (_, proposal) = self.proposal_row(owner, id).await?;
        match proposal.state.as_str() {
            "proposed" => {}
            "applied" => return self.machine_setup_proposal(owner, id).await,
            "superseded" => return Err(ApiError::conflict("machine_setup_proposal_superseded")),
            _ => return Err(ApiError::conflict("machine_setup_proposal_discarded")),
        }
        let machine = self.machine(owner).await?;
        if machine.recipe.setup_script.trim() != proposal.current_setup_script.trim() {
            return Err(ApiError::conflict("machine_setup_proposal_stale"));
        }
        let current = {
            let mut connection = self.pool.acquire().await?;
            repositories(&mut connection, owner).await?
        };
        let (added, _) = proposal.repository_changes();
        let added: Vec<&str> = added
            .into_iter()
            .filter(|identity| !current.iter().any(|r| r.identity == *identity))
            .collect();
        let removed: Vec<&Repository> = current
            .iter()
            .filter(|r| !proposal.repositories.contains(&r.identity))
            .collect();
        // Resolve every new repository before changing anything.
        let mut connects = Vec::new();
        if !added.is_empty() {
            let github = self
                .github_native
                .as_ref()
                .filter(|g| g.enabled(owner))
                .ok_or_else(|| ApiError::conflict("machine_setup_github_required"))?;
            let catalog = github.catalog(access).await?;
            for identity in &added {
                let repository = catalog
                    .repositories
                    .iter()
                    .find(|r| {
                        setup::repository_identity(&r.name_with_owner).as_deref() == Some(identity)
                    })
                    .ok_or_else(|| ApiError::conflict("machine_setup_repository_unavailable"))?;
                connects.push(zuse_protocol::cloud_projects::ProjectConnectRequest {
                    repository_url: repository.https_url.clone(),
                    default_branch: repository.default_branch.clone(),
                    visibility: if repository.is_private {
                        zuse_protocol::cloud_projects::ProjectVisibility::Private
                    } else {
                        zuse_protocol::cloud_projects::ProjectVisibility::Public
                    },
                    display_name: None,
                    cloud_environment: Default::default(),
                    secret_bindings: vec![],
                    idempotency_key: format!("{id}:{identity}"),
                });
            }
        }
        for request in connects {
            self.project_connect(owner, request, now).await?;
        }
        for repository in removed {
            self.project_remove(owner, &repository.project_id, now)
                .await?;
        }
        let mut revision = machine.revision;
        if proposal.script_changed() {
            let mut recipe = machine.recipe.clone();
            recipe.setup_script = proposal.setup_script.clone();
            revision = self
                .save_machine_scoped(
                    access,
                    SaveMachine {
                        expected_revision: machine.revision,
                        recipe,
                    },
                    now,
                )
                .await?
                .revision;
        }
        self.decide(owner, id, "applied", Some(revision), now)
            .await?;
        // The normal "Rebuild now": a fresh build even when nothing changed.
        match self.rebuild_machine_scoped(access, now).await {
            Err(error) if error.code == "machine_setup_script_required" => {}
            other => {
                other?;
            }
        }
        self.machine_setup_proposal(owner, id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_instructions_come_after_the_accounts_own_and_fit_the_limit() {
        let delivery = with_setup_instructions(None);
        assert_eq!(delivery.instructions, setup::INSTRUCTIONS);
        assert_eq!(delivery.validate(), Ok(()));
        let own = MachineDelivery {
            revision: 4,
            instructions: "Keep commits small.".into(),
            skills: vec![],
            files: vec![],
        };
        let delivery = with_setup_instructions(Some(own));
        assert_eq!(delivery.revision, 4);
        assert!(
            delivery
                .instructions
                .starts_with("Keep commits small.\n\n# You are")
        );
        let long = MachineDelivery {
            revision: 1,
            instructions: "é".repeat(40 * 1024),
            skills: vec![],
            files: vec![],
        };
        let delivery = with_setup_instructions(Some(long));
        assert!(delivery.instructions.ends_with(setup::INSTRUCTIONS));
        assert_eq!(delivery.validate(), Ok(()));
    }
}
