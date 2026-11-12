//! In-place runtime updates end to end against the restricted role: an
//! operator registers signed releases, a message that needs a newer runtime
//! demands one, the runtime polls, swaps and attests, and the same message is
//! then admitted.
use super::runtime_outdated::{admitted, computer, enqueued, run};
use super::*;
use zuse_protocol::{cloud_runtime_updates as u, cloud_task_commands as w};

const FILE: &str = "/home/zuse/.zuse-data/uploads/a/shot.png";
const CREDENTIAL: &str = "workspace_runtime_fixture_credential";

fn sha(c: char) -> String {
    c.to_string().repeat(64)
}

async fn register(
    owner: &PgPool,
    key: &ed25519_dalek::SigningKey,
    sha: &str,
    version: &str,
    names: Option<Vec<&str>>,
    now: i64,
) {
    let mut file = crate::runtime_releases::signed_test_file(key, sha, version);
    file.bootstrap_capabilities = names.map(|n| n.into_iter().map(str::to_owned).collect());
    file.register(
        owner,
        &key.verifying_key(),
        crate::runtime_releases::ReleaseState::Stable,
        now,
    )
    .await
    .unwrap();
}

/// Give the computer a live runtime credential and a pinned runtime.
async fn attach_runtime(owner: &PgPool, id: &str, runtime: &str, now: i64) {
    let hash: String = Sha256::digest(CREDENTIAL.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    sqlx::query("UPDATE api_cloud_workspaces SET runtime_credential_hash=$2,request_config=request_config||jsonb_build_object('runtimeCredentialExpiresAtMs',$3::bigint,'runtimeRelease',jsonb_build_object('sha256',$4::text,'version','1.0.0','appliedAt',0)) WHERE workspace_id=$1")
        .bind(id)
        .bind(hash)
        .bind(now + 3_600_000)
        .bind(runtime)
        .execute(owner)
        .await
        .unwrap();
}

async fn config(pool: &PgPool, id: &str) -> Value {
    sqlx::query_scalar("SELECT request_config FROM api_cloud_workspaces WHERE workspace_id=$1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn status(control: &RuntimeControl, pool: &PgPool, id: &str) -> u::UpdateStatus {
    let row: Value =
        sqlx::query_scalar("SELECT to_jsonb(w) FROM api_cloud_workspaces w WHERE workspace_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    control.runtime_update_status(&row).await.unwrap()
}

fn attestation(
    outcome: u::Outcome,
    update_id: Option<String>,
    serving: &str,
    capabilities: &[Value],
) -> u::RuntimeAttestation {
    u::RuntimeAttestation {
        update_id,
        outcome,
        sha256: serving.into(),
        version: "1.1.0".into(),
        capabilities: capabilities
            .iter()
            .map(|c| c.as_str().unwrap().to_owned())
            .collect(),
        previous_sha256: None,
        error_code: (outcome != u::Outcome::Committed).then(|| "runtime_update_rolled_back".into()),
    }
}

pub(super) async fn exercise(
    control: &std::sync::Arc<RuntimeControl>,
    owner: &PgPool,
    pool: &PgPool,
    access: &crate::access::Access,
    master: &crate::crypto::MasterKey,
    plan: impl Fn(&str) -> Value,
    now: i64,
) {
    let key = ed25519_dalek::SigningKey::from_bytes(&[61; 32]);
    let (old, new) = (sha('1'), sha('2'));
    // The application role reads the registry but can never write it.
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT has_table_privilege(current_user,'zuse_rust_runtime_releases','INSERT')"
        )
        .fetch_one(pool)
        .await
        .unwrap()
    );
    // A release signed by any other key is refused before anything is written.
    let forged = crate::runtime_releases::signed_test_file(
        &ed25519_dalek::SigningKey::from_bytes(&[62; 32]),
        &new,
        "1.1.0",
    );
    let refused = forged
        .register(
            owner,
            &key.verifying_key(),
            crate::runtime_releases::ReleaseState::Stable,
            now,
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, "runtime_release_signature_invalid");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM zuse_rust_runtime_releases")
            .fetch_one(pool)
            .await
            .unwrap(),
        0
    );
    register(owner, &key, &old, "1.0.0", None, now - 10).await;
    register(
        owner,
        &key,
        &new,
        "1.1.0",
        Some(vec![
            "rust-rpc-v1",
            w::DIRECT_UPLOADS_CAPABILITY,
            w::MESSAGE_ACCESS_MODE_CAPABILITY,
            u::CAPABILITY,
        ]),
        now,
    )
    .await;
    // Registration is idempotent; a changed release under the same digest is not.
    register(
        owner,
        &key,
        &new,
        "1.1.0",
        Some(vec![
            "rust-rpc-v1",
            w::DIRECT_UPLOADS_CAPABILITY,
            w::MESSAGE_ACCESS_MODE_CAPABILITY,
            u::CAPABILITY,
        ]),
        now,
    )
    .await;
    let mut changed = crate::runtime_releases::signed_test_file(&key, &new, "1.1.0");
    changed.compressed_size_bytes += 1;
    assert_eq!(
        changed
            .register(
                owner,
                &key.verifying_key(),
                crate::runtime_releases::ReleaseState::Stable,
                now,
            )
            .await
            .unwrap_err()
            .code,
        "runtime_release_immutable"
    );

    let old_caps = vec![
        json!("rust-rpc-v1"),
        json!("rust-session-commands-v1"),
        json!(w::CAPABILITY),
        json!(u::CAPABILITY),
    ];
    let mut new_caps = old_caps.clone();
    new_caps.push(json!(w::DIRECT_UPLOADS_CAPABILITY));
    new_caps.push(json!(w::MESSAGE_ACCESS_MODE_CAPABILITY));
    new_caps.push(json!("capability-this-server-does-not-know-v1"));

    // 1. Demand, offer, a rollback, a retry, commit, then the message runs.
    let id = "runtime-updatable";
    computer(
        control,
        pool,
        access,
        master,
        plan(id),
        Some(old_caps.clone()),
        now,
    )
    .await;
    attach_runtime(owner, id, &old, now).await;
    assert_eq!(
        status(control, pool, id).await.state,
        u::UpdateState::Pending
    );
    let before = enqueued(pool, id).await;
    let files = format!("{id}-files");
    let refused = control
        .task_command_submit(id, access, run(id, &files, &[FILE], false), now)
        .await
        .unwrap_err();
    assert_eq!(refused.code, u::RUNTIME_UPDATING);
    assert_eq!(refused.status, axum::http::StatusCode::CONFLICT);
    assert!(!admitted(pool, id, &files).await);
    assert_eq!(enqueued(pool, id).await, before);
    let urgency: (String, String) =
        sqlx::query_as("SELECT urgency,state FROM zuse_rust_runtime_updates WHERE workspace_id=$1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(urgency, ("now".into(), "pending".into()));
    let pending = status(control, pool, id).await;
    assert_eq!(pending.state, u::UpdateState::Pending);
    assert!(pending.update_id.is_some());

    let offer = control
        .runtime_update_offer(id, CREDENTIAL, now + 1)
        .await
        .unwrap();
    let directive = offer.directive.expect("an update is offered");
    assert_eq!(directive.urgency, u::Urgency::Now);
    assert_eq!(Some(&directive.update_id), pending.update_id.as_ref());
    assert_eq!(directive.artifact.release.sha256, new);
    assert_eq!(directive.artifact.release.version, "1.1.0");
    assert!(directive.artifact.url.starts_with(&format!(
        "https://account.r2.example.test/zuse-runtime/runtime/x86_64/{new}.zst?"
    )));
    assert_eq!(
        directive.artifact.url_expires_at,
        (now + 1) / 1000 * 1000 + 600_000
    );
    // The signature in the offer verifies with the release key.
    use base64::Engine as _;
    let signature: [u8; 64] = base64::engine::general_purpose::STANDARD
        .decode(&directive.artifact.signature)
        .unwrap()
        .try_into()
        .unwrap();
    key.verifying_key()
        .verify_strict(
            &directive.artifact.release.signing_bytes(),
            &ed25519_dalek::Signature::from_bytes(&signature),
        )
        .unwrap();
    assert_eq!(
        status(control, pool, id).await.state,
        u::UpdateState::Downloading
    );
    // A wrong credential is fenced like every runtime route.
    assert_eq!(
        control
            .runtime_update_offer(id, "workspace_runtime_wrong", now + 1)
            .await
            .unwrap_err()
            .code,
        "workspace_runtime_fenced"
    );

    control
        .runtime_update_attest(
            id,
            CREDENTIAL,
            attestation(
                u::Outcome::RolledBack,
                Some(directive.update_id.clone()),
                &old,
                &old_caps,
            ),
            now + 2,
        )
        .await
        .unwrap();
    let rolled = status(control, pool, id).await;
    assert_eq!(rolled.state, u::UpdateState::RolledBack);
    assert_eq!(
        rolled.error_code.as_deref(),
        Some("runtime_update_rolled_back")
    );
    // A retry is a new background attempt.
    let retry = control
        .runtime_update_offer(id, CREDENTIAL, now + 3)
        .await
        .unwrap()
        .directive
        .unwrap();
    assert_ne!(retry.update_id, directive.update_id);
    assert_eq!(retry.urgency, u::Urgency::Background);

    control
        .runtime_update_attest(
            id,
            CREDENTIAL,
            attestation(
                u::Outcome::Committed,
                Some(retry.update_id.clone()),
                &new,
                &new_caps,
            ),
            now + 4,
        )
        .await
        .unwrap();
    let stored = config(pool, id).await;
    assert_eq!(stored["runtimeRelease"]["sha256"], new);
    assert_eq!(stored["runtimeRelease"]["version"], "1.1.0");
    assert_eq!(stored["runtimeRelease"]["previousSha256"], old);
    assert_eq!(stored["runtimeRelease"]["appliedAt"], now + 4);
    assert_eq!(
        stored["runtimeBootstrapReceipt"]["capabilities"],
        json!(new_caps)
    );
    // The bootstrap fence is untouched.
    assert_eq!(stored["runtimeBootstrapReceipt"]["workspaceId"], id);
    let committed = status(control, pool, id).await;
    assert_eq!(committed.state, u::UpdateState::Committed);
    assert_eq!(committed.update_id, Some(retry.update_id));
    assert!(
        control
            .runtime_update_offer(id, CREDENTIAL, now + 5)
            .await
            .unwrap()
            .directive
            .is_none()
    );
    let again = format!("{id}-files-after-update");
    control
        .task_command_submit(id, access, run(id, &again, &[FILE], true), now + 6)
        .await
        .unwrap();
    assert!(admitted(pool, id, &again).await);
    assert_eq!(enqueued(pool, id).await, before + 1);
    // An attestation for a digest nobody registered is refused.
    assert_eq!(
        control
            .runtime_update_attest(
                id,
                CREDENTIAL,
                attestation(u::Outcome::Committed, None, &sha('9'), &new_caps),
                now + 7,
            )
            .await
            .unwrap_err()
            .code,
        "cloud_runtime_release_unknown"
    );

    // 2. Update support but already on the newest release: nothing to install.
    let id = "runtime-updatable-on-newest";
    computer(
        control,
        pool,
        access,
        master,
        plan(id),
        Some(old_caps.clone()),
        now,
    )
    .await;
    attach_runtime(owner, id, &new, now).await;
    let refused = control
        .task_command_submit(
            id,
            access,
            run(id, &format!("{id}-files"), &[FILE], false),
            now,
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, u::RUNTIME_UPDATE_UNAVAILABLE);
    assert_eq!(
        status(control, pool, id).await.state,
        u::UpdateState::Current
    );

    // 3. No update support at all.
    let id = "runtime-not-updatable";
    let mut without = old_caps.clone();
    without.retain(|c| c != u::CAPABILITY);
    computer(control, pool, access, master, plan(id), Some(without), now).await;
    attach_runtime(owner, id, &old, now).await;
    let refused = control
        .task_command_submit(id, access, run(id, &format!("{id}-mode"), &[], true), now)
        .await
        .unwrap_err();
    assert_eq!(refused.code, u::RUNTIME_UPDATE_UNAVAILABLE);
    assert!(
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT urgency FROM zuse_rust_runtime_updates WHERE workspace_id=$1"
        )
        .bind(id)
        .fetch_optional(pool)
        .await
        .unwrap()
        .is_none()
    );
    let unsupported = status(control, pool, id).await;
    assert_eq!(unsupported.state, u::UpdateState::Unavailable);
    assert!(
        control
            .runtime_update_offer(id, CREDENTIAL, now)
            .await
            .unwrap()
            .directive
            .is_none()
    );

    // 4. Two failed attempts at one release stop it being offered.
    let id = "runtime-update-exhausted";
    computer(
        control,
        pool,
        access,
        master,
        plan(id),
        Some(old_caps.clone()),
        now,
    )
    .await;
    attach_runtime(owner, id, &old, now).await;
    for attempt in 0..2 {
        let offered = control
            .runtime_update_offer(id, CREDENTIAL, now + attempt)
            .await
            .unwrap()
            .directive
            .unwrap();
        let report = attestation(u::Outcome::Failed, Some(offered.update_id), &old, &old_caps);
        control
            .runtime_update_attest(id, CREDENTIAL, report.clone(), now + attempt)
            .await
            .unwrap();
        // A duplicate report is not a second failure.
        control
            .runtime_update_attest(id, CREDENTIAL, report, now + attempt)
            .await
            .unwrap();
    }
    assert!(
        control
            .runtime_update_offer(id, CREDENTIAL, now + 3)
            .await
            .unwrap()
            .directive
            .is_none()
    );
    let exhausted = status(control, pool, id).await;
    assert_eq!(exhausted.state, u::UpdateState::Unavailable);
    assert_eq!(
        exhausted.error_code.as_deref(),
        Some("runtime_update_rolled_back")
    );
    let refused = control
        .task_command_submit(
            id,
            access,
            run(id, &format!("{id}-files"), &[FILE], false),
            now,
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code, u::RUNTIME_UPDATE_UNAVAILABLE);
}
