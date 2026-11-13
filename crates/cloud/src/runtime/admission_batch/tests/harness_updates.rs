//! Harness bundle offers end to end against the restricted role: an operator
//! registers signed bundles, the runtime polls, adopts or fails, attests, and
//! the server keeps channels, compatibility pins, failures and rollback.
use super::runtime_outdated::computer;
use super::runtime_updates::{CREDENTIAL, attach_runtime, sha};
use super::*;
use crate::harness_bundles::{BundleState, signed_test_file};
use zuse_protocol::cloud_harness_updates as h;

fn image(agent: &str, version: &str) -> h::AgentHarness {
    h::AgentHarness {
        agent: agent.into(),
        adapters: vec![h::adapter_for(agent).unwrap().into()],
        current: Some(h::HarnessBinding {
            sha256: None,
            version: version.into(),
            state_format: "image-tested-v1".into(),
        }),
    }
}
fn on(agent: &str, sha: &str, version: &str) -> h::AgentHarness {
    h::AgentHarness {
        current: Some(h::HarnessBinding {
            sha256: Some(sha.into()),
            version: version.into(),
            state_format: format!("{agent}-sessions-v1"),
        }),
        ..image(agent, version)
    }
}
fn poll(agents: Vec<h::AgentHarness>) -> h::HarnessPoll {
    h::HarnessPoll {
        architecture: "x86_64".into(),
        agents,
    }
}
fn attest(
    outcome: h::HarnessOutcome,
    directive: &h::HarnessDirective,
    code: Option<&str>,
) -> h::HarnessAttestation {
    h::HarnessAttestation {
        update_id: Some(directive.update_id.clone()),
        outcome,
        agent: directive.artifact.bundle.agent.clone(),
        sha256: directive.artifact.bundle.sha256.clone(),
        version: directive.artifact.bundle.version.clone(),
        previous_sha256: None,
        error_code: code.map(str::to_owned),
    }
}
fn for_agent<'a>(offer: &'a h::HarnessOffer, agent: &str) -> Option<&'a h::HarnessDirective> {
    offer
        .directives
        .iter()
        .find(|d| d.artifact.bundle.agent == agent)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn exercise(
    control: &std::sync::Arc<RuntimeControl>,
    canary: &std::sync::Arc<RuntimeControl>,
    owner: &PgPool,
    pool: &PgPool,
    access: &crate::access::Access,
    master: &crate::crypto::MasterKey,
    plan: impl Fn(&str) -> Value,
    now: i64,
) {
    let key = ed25519_dalek::SigningKey::from_bytes(&[71; 32]);
    let register = |file: crate::harness_bundles::BundleFile, state: BundleState, at: i64| {
        let key = key.verifying_key();
        async move { file.register(owner, &key, state, at).await }
    };
    // The application role reads the registry but can never write it.
    assert!(
        !sqlx::query_scalar::<_, bool>(
            "SELECT has_table_privilege(current_user,'zuse_rust_harness_bundles','INSERT')"
        )
        .fetch_one(pool)
        .await
        .unwrap()
    );
    let (claude_a, claude_b, codex_c) = (sha('a'), sha('b'), sha('c'));
    let forged = signed_test_file(
        &ed25519_dalek::SigningKey::from_bytes(&[72; 32]),
        "claude",
        &claude_a,
        "2.1.312",
    );
    assert_eq!(
        register(forged, BundleState::Stable, now)
            .await
            .unwrap_err()
            .code,
        "harness_bundle_signature_invalid"
    );
    let a = signed_test_file(&key, "claude", &claude_a, "2.1.312");
    register(a.clone(), BundleState::Stable, now - 30)
        .await
        .unwrap();
    // Idempotent; a changed bundle under the same digest is refused.
    register(a.clone(), BundleState::Stable, now - 30)
        .await
        .unwrap();
    let mut changed = a.clone();
    changed.compressed_size_bytes += 1;
    assert_eq!(
        register(changed, BundleState::Stable, now)
            .await
            .unwrap_err()
            .code,
        "harness_bundle_immutable"
    );
    register(
        signed_test_file(&key, "claude", &claude_b, "2.1.313"),
        BundleState::Staging,
        now - 20,
    )
    .await
    .unwrap();
    register(
        signed_test_file(&key, "codex", &codex_c, "0.166.0"),
        BundleState::Stable,
        now - 20,
    )
    .await
    .unwrap();

    // A runtime without harness updates is offered nothing.
    let old = "harness-old-runtime";
    computer(
        control,
        pool,
        access,
        master,
        plan(old),
        Some(vec![json!("rust-rpc-v1")]),
        now,
    )
    .await;
    attach_runtime(owner, old, &sha('1'), now).await;
    let none = control
        .harness_update_poll(old, CREDENTIAL, poll(vec![image("claude", "2.1.300")]), now)
        .await
        .unwrap();
    assert!(none.directives.is_empty());
    // A wrong credential is refused.
    assert!(
        control
            .harness_update_poll(old, "wrong", poll(vec![]), now)
            .await
            .is_err()
    );

    let id = "harness-updatable";
    computer(
        control,
        pool,
        access,
        master,
        plan(id),
        Some(vec![json!("rust-rpc-v1"), json!(h::CAPABILITY)]),
        now,
    )
    .await;
    attach_runtime(owner, id, &sha('1'), now).await;

    // 1. Stable channel: claude moves to the stable bundle (not the staging
    // one), codex to its bundle; the image CLIs are recorded.
    let agents = || poll(vec![image("claude", "2.1.300"), image("codex", "0.165.0")]);
    let offer = control
        .harness_update_poll(id, CREDENTIAL, agents(), now)
        .await
        .unwrap();
    let claude = for_agent(&offer, "claude").expect("claude offered");
    assert_eq!(claude.artifact.bundle.sha256, claude_a);
    assert!(claude.artifact.url.starts_with(&format!(
        "https://account.r2.example.test/zuse-runtime/runtime/harness/claude/x86_64/{claude_a}.tar.zst?"
    )));
    use base64::Engine as _;
    let signature: [u8; 64] = base64::engine::general_purpose::STANDARD
        .decode(&claude.artifact.signature)
        .unwrap()
        .try_into()
        .unwrap();
    key.verifying_key()
        .verify_strict(
            &claude.artifact.bundle.signing_bytes(),
            &ed25519_dalek::Signature::from_bytes(&signature),
        )
        .unwrap();
    let codex = for_agent(&offer, "codex").expect("codex offered").clone();
    let bindings: Vec<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT agent,sha256,version FROM zuse_rust_harness_bindings WHERE workspace_id=$1 ORDER BY agent",
    )
    .bind(id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(
        bindings,
        vec![
            ("claude".into(), None, "2.1.300".into()),
            ("codex".into(), None, "0.165.0".into())
        ]
    );
    // The same in-flight offer keeps its id.
    let again = control
        .harness_update_poll(id, CREDENTIAL, agents(), now + 1)
        .await
        .unwrap();
    assert_eq!(
        for_agent(&again, "claude").unwrap().update_id,
        claude.update_id
    );

    // 2. Two failures stop offering that bundle to this computer; a stale
    // report changes nothing.
    let mut stale = attest(
        h::HarnessOutcome::Failed,
        &codex,
        Some("harness_update_probe_failed"),
    );
    stale.update_id = Some("stale".into());
    control
        .harness_update_attest(id, CREDENTIAL, stale, now + 2)
        .await
        .unwrap();
    let mut directive = codex.clone();
    for attempt in 0..2 {
        control
            .harness_update_attest(
                id,
                CREDENTIAL,
                attest(
                    h::HarnessOutcome::Failed,
                    &directive,
                    Some("harness_update_probe_failed"),
                ),
                now + 3 + attempt,
            )
            .await
            .unwrap();
        let next = control
            .harness_update_poll(id, CREDENTIAL, agents(), now + 5 + attempt)
            .await
            .unwrap();
        match for_agent(&next, "codex") {
            Some(retry) => {
                assert_eq!(attempt, 0, "offered again after the second failure");
                assert_ne!(retry.update_id, directive.update_id);
                directive = retry.clone();
            }
            None => assert_eq!(attempt, 1),
        }
    }

    // 3. Adoption is recorded as the computer's binding.
    let unknown = h::HarnessAttestation {
        sha256: sha('9'),
        ..attest(h::HarnessOutcome::Adopted, claude, None)
    };
    assert_eq!(
        control
            .harness_update_attest(id, CREDENTIAL, unknown, now + 9)
            .await
            .unwrap_err()
            .code,
        "harness_bundle_unknown"
    );
    control
        .harness_update_attest(
            id,
            CREDENTIAL,
            attest(h::HarnessOutcome::Adopted, claude, None),
            now + 10,
        )
        .await
        .unwrap();
    let bound: (Option<String>, String, String, Option<i64>) = sqlx::query_as("SELECT sha256,version,state_format,adopted_at FROM zuse_rust_harness_bindings WHERE workspace_id=$1 AND agent='claude'")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(
        bound,
        (
            Some(claude_a.clone()),
            "2.1.312".into(),
            "claude-sessions-v1".into(),
            Some(now + 10)
        )
    );
    let state: String = sqlx::query_scalar(
        "SELECT state FROM zuse_rust_harness_updates WHERE workspace_id=$1 AND agent='claude'",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(state, "adopted");
    let current = || poll(vec![on("claude", &claude_a, "2.1.312")]);
    assert!(
        control
            .harness_update_poll(id, CREDENTIAL, current(), now + 11)
            .await
            .unwrap()
            .directives
            .is_empty()
    );

    // 4. A newer bundle that cannot continue the current session state is
    // never offered: the computer stays pinned.
    let mut incompatible = signed_test_file(&key, "claude", &sha('d'), "2.1.400");
    incompatible.bundle.state_format = "claude-sessions-v2".into();
    incompatible.bundle.reads_state_formats = vec!["claude-sessions-v2".into()];
    incompatible.signature = base64::engine::general_purpose::STANDARD
        .encode(ed25519_dalek::Signer::sign(&key, &incompatible.bundle.signing_bytes()).to_bytes());
    register(incompatible, BundleState::Stable, now + 12)
        .await
        .unwrap();
    assert!(
        control
            .harness_update_poll(id, CREDENTIAL, current(), now + 13)
            .await
            .unwrap()
            .directives
            .is_empty()
    );

    // 5. Retiring the bundle a computer runs rolls it back to the newest good
    // stable bundle, even an older one.
    register(
        signed_test_file(&key, "claude", &sha('e'), "2.1.311"),
        BundleState::Stable,
        now + 14,
    )
    .await
    .unwrap();
    assert!(
        control
            .harness_update_poll(id, CREDENTIAL, current(), now + 15)
            .await
            .unwrap()
            .directives
            .is_empty()
    );
    crate::harness_bundles::set_state(owner, &claude_a, BundleState::Retired, now + 16)
        .await
        .unwrap();
    let rollback = control
        .harness_update_poll(id, CREDENTIAL, current(), now + 17)
        .await
        .unwrap();
    assert_eq!(
        for_agent(&rollback, "claude")
            .unwrap()
            .artifact
            .bundle
            .sha256,
        sha('e')
    );

    // 6. Canary accounts on a stable deployment also take canary bundles.
    crate::harness_bundles::set_state(owner, &claude_b, BundleState::Canary, now + 18)
        .await
        .unwrap();
    let image_claude = || poll(vec![image("claude", "2.1.300")]);
    assert_eq!(
        for_agent(
            &control
                .harness_update_poll(id, CREDENTIAL, image_claude(), now + 19)
                .await
                .unwrap(),
            "claude"
        )
        .unwrap()
        .artifact
        .bundle
        .sha256,
        sha('e')
    );
    assert_eq!(
        for_agent(
            &canary
                .harness_update_poll(id, CREDENTIAL, image_claude(), now + 20)
                .await
                .unwrap(),
            "claude"
        )
        .unwrap()
        .artifact
        .bundle
        .sha256,
        claude_b
    );
    assert_eq!(
        crate::harness_bundles::set_state(owner, &sha('7'), BundleState::Stable, now)
            .await
            .unwrap_err()
            .code,
        "harness_bundle_not_found"
    );
}
