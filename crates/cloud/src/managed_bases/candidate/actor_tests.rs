use super::*;
fn artifact() -> ManagedBaseCandidate {
    let mut c = super::super::tests::candidate();
    c.manifest.rust_organization_execution_version = Some(1);
    c
}
#[test]
fn personal_candidate_default_keeps_single_owner_and_wire_shape() {
    let c = artifact();
    let p = c.pin("user_actor", 2000, true).unwrap();
    assert!(serde_json::to_value(&p).unwrap().get("actorId").is_none());
    p.deployment(
        "staging",
        &["user_actor".into()],
        &["user_actor".into()],
        1000,
    )
    .unwrap();
    assert!(p.require_actor("other").is_err());
    p.require_actor("user_actor").unwrap();
    assert!(c.pin_for_actor("user_actor", "other", 2000, true).is_err());
}
#[test]
fn organization_candidate_requires_exact_actor_owner_and_two_credential_scopes() {
    let c = artifact();
    let owner = "organization:org_fixture";
    let p = c.pin_for_actor(owner, "user_actor", 2000, true).unwrap();
    p.deployment(
        "staging",
        &[owner.into()],
        &[owner.into(), "user_actor".into()],
        1000,
    )
    .unwrap();
    p.deployment(
        "staging",
        &[owner.into()],
        &["user_actor".into(), owner.into()],
        1000,
    )
    .unwrap();
    for auth in [
        vec![owner.into()],
        vec!["user_actor".into()],
        vec![owner.into(), "unrelated".into()],
        vec![owner.into(), "user_actor".into(), "unrelated".into()],
    ] {
        assert!(
            p.deployment("staging", &[owner.into()], &auth, 1000)
                .is_err()
        );
    }
    assert!(
        p.deployment(
            "production",
            &[owner.into()],
            &[owner.into(), "user_actor".into()],
            1000
        )
        .is_err()
    );
    assert!(
        p.deployment(
            "staging",
            &[owner.into(), "user_actor".into()],
            &[owner.into(), "user_actor".into()],
            1000
        )
        .is_err()
    );
    assert!(
        p.deployment(
            "staging",
            &[owner.into()],
            &[owner.into(), "user_actor".into()],
            2000
        )
        .is_err()
    );
    p.require_actor("user_actor").unwrap();
    assert!(p.require_actor("other").is_err());
    let mut missing = p.clone();
    missing.actor_id = None;
    assert!(missing.validate().is_err());
    let mut unsupported = c.clone();
    unsupported.manifest.rust_organization_execution_version = None;
    assert!(
        unsupported
            .pin_for_actor(owner, "user_actor", 2000, true)
            .is_err()
    );
}
#[test]
fn organization_candidate_binds_the_existing_native_task_provenance() {
    let c = artifact();
    let p = c
        .pin_for_actor("organization:org_fixture", "user_actor", 2000, true)
        .unwrap();
    let mut config = c.settings(&p);
    config["rustActorId"] = json!("user_actor");
    config["rustOrganizationExecutionVersion"] = json!(1);
    config["agentConnectionId"] = json!("personal-connection");
    config["agentConnectionOwnerId"] = json!("user_actor");
    config["providerAuthMode"] = json!("broker-v1");
    config["sharingPolicy"] =
        json!({"creatorSubject":"user_actor","creatorMembershipId":"membership-user_actor"});
    let row = json!({"account_id":p.owner_id,"provider":"boxd","request_config":config});
    p.row_binding(&row).unwrap();
    for (key, value) in [
        ("rustActorId", json!("other")),
        ("agentConnectionOwnerId", json!("other")),
        ("providerAuthMode", json!("legacy")),
        ("rustOrganizationExecutionVersion", json!(0)),
    ] {
        let mut changed = row.clone();
        changed["request_config"][key] = value;
        assert!(p.row_binding(&changed).is_err(), "{key}");
    }
    let mut changed = row.clone();
    changed["request_config"]["sharingPolicy"]["creatorMembershipId"] = Value::Null;
    assert!(p.row_binding(&changed).is_err());
    let mut changed = row;
    changed["request_config"]["agentConnectionOwnerId"] = json!(p.owner_id);
    p.row_binding(&changed).unwrap();
}
