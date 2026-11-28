//! Setup chat routes. The setup computer's runtime reads the image
//! configuration and stores proposals; the owner's apps read, apply and
//! discard them. Applying or discarding puts the setup computer to sleep: its
//! work is done, and a sleeping computer costs nothing.
use super::*;
use zuse_protocol::machine_setup::{ProposalView, Request};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RuntimeRequest {
    chat_id: String,
    request: Request,
}

pub(super) async fn runtime_request(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(workspace): Path<String>,
    body: Result<Json<RuntimeRequest>, JsonRejection>,
) -> Result<Json<Value>, ApiError> {
    let Json(input) = body.map_err(|_| ApiError::bad_request("invalid_machine_setup_request"))?;
    let control = runtime(&state)?;
    let owner = control
        .machine_setup_owner(&workspace, &input.chat_id, runtime_bearer(&headers)?)
        .await?;
    let value = match input.request {
        Request::Get => json!(state.store.machine_setup_config(&owner).await?),
        Request::Propose {
            setup_script,
            repositories,
            notes,
        } => json!(
            state
                .store
                .machine_setup_propose(
                    &control.shared_pool(),
                    &owner,
                    &workspace,
                    &input.chat_id,
                    setup_script,
                    repositories,
                    notes,
                    chrono::Utc::now().timestamp_millis(),
                )
                .await?
        ),
    };
    Ok(Json(value))
}

pub(super) async fn proposal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ProposalView>, ApiError> {
    let access = state.access(&headers, AccessKind::Content).await?;
    Ok(Json(
        state
            .store
            .machine_setup_proposal(&access.owner_id, &id)
            .await?,
    ))
}

pub(super) async fn apply(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ProposalView>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let now = chrono::Utc::now().timestamp_millis();
    let view = state.store.apply_machine_setup(&access, &id, now).await?;
    sleep_setup_computer(&state, &access, &id, now).await;
    Ok(Json(view))
}

pub(super) async fn discard(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ProposalView>, ApiError> {
    let access = state.access(&headers, AccessKind::Administration).await?;
    let now = chrono::Utc::now().timestamp_millis();
    let view = state.store.discard_machine_setup(&access, &id, now).await?;
    sleep_setup_computer(&state, &access, &id, now).await;
    Ok(Json(view))
}

/// Best effort: idle sleep remains the backstop when this fails.
async fn sleep_setup_computer(
    state: &AppState,
    access: &crate::access::Access,
    proposal: &str,
    now: i64,
) {
    let Some(runtime) = state.runtime.as_ref() else {
        return;
    };
    let workspace = match state
        .store
        .machine_setup_workspace(&access.owner_id, proposal)
        .await
    {
        Ok(workspace) => workspace,
        Err(error) => {
            tracing::warn!(code = error.code, "Setup computer not found to sleep");
            return;
        }
    };
    if let Err(error) = runtime
        .lifecycle_action(
            &workspace,
            access,
            zuse_protocol::cloud_lifecycle::Action::Pause,
            None,
            now,
        )
        .await
    {
        tracing::info!(code = error.code, "Setup computer left to idle sleep");
    }
}
