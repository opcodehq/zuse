// Included by the isolated native authority fixture; no live identities or sockets.
async fn departure_closes_gateway_and_rejects_queued_author(
    pool: &PgPool,
    authority: &Arc<NativeGithub>,
    fixture: &Arc<Fixture>,
    original_config: &Value,
    public_api: &Router,
) {
    use axum::response::IntoResponse;
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;
    use tower::ServiceExt;
    use zuse_protocol::cloud_commands as commands;
    use zuse_protocol::cloud_gateway::{RUST_PROTOCOL, RUST_RPC};
    // A separate authority object represents another API/worker process. Its
    // local membership counter never sees the accepted webhook.
    let source = &authority.config;
    let independent = Arc::new(NativeGithub {
        membership_epoch: std::sync::atomic::AtomicU64::new(0),
        observation_gates: Default::default(),
            fast_start_accounts: Default::default(),
        pool: pool.clone(),
        master: MasterKey::parse(&URL_SAFE_NO_PAD.encode([73; 32])).unwrap(),
        accounts: authority.accounts.clone(),
        http: authority.http.clone(),
        config: Config {
            origin: source.origin.clone(),
            app_id: source.app_id.clone(),
            slug: source.slug.clone(),
            client_id: source.client_id.clone(),
            client_secret: source.client_secret.clone(),
            webhook_secret: source.webhook_secret.clone(),
            signing_key: source.signing_key.clone(),
            api_origin: source.api_origin.clone(),
            oauth_origin: source.oauth_origin.clone(),
        },
    });
    let signing = ed25519_dalek::SigningKey::from_bytes(&[71; 32]);
    let jwk = json!({"kty":"OKP","crv":"Ed25519","d":URL_SAFE_NO_PAD.encode([71;32]),"x":URL_SAFE_NO_PAD.encode(signing.verifying_key().as_bytes())});
    let control = Arc::new(crate::runtime::RuntimeControl::from_pool(
        pool.clone(),
        MasterKey::parse(&URL_SAFE_NO_PAD.encode([73; 32])).unwrap(),
        crate::tickets::TicketSigner::from_jwk(&jwk.to_string(), "https://api.example.test")
            .unwrap(),
    ));
    control
        .configure_scope_authority(
            Arc::new(BrowserIdentity),
            Some(independent.clone()),
            vec!["team".into()],
        )
        .unwrap();
    for migration in [
        include_str!("../../../migrations/0002_gateway_leases.sql"),
        include_str!("../../../migrations/0003_command_mailbox.sql"),
    ] {
        sqlx::raw_sql(migration).execute(pool).await.unwrap();
    }
    sqlx::query("ALTER TABLE api_cloud_workspaces ADD COLUMN runtime_state text, ADD COLUMN failure_reason text").execute(pool).await.unwrap();
    let mut config = original_config.clone();
    config["rustActorId"] = json!("actor-two");
    config["rustCommandMailboxEnabled"] = json!(true);
    config["rustStorageEnrollment"]["phase"] = json!("committed");
    config["sharingPolicy"] = json!({"audience":"organization","permission":"edit","creatorSubject":"actor-two","creatorMembershipId":"membership-actor-two","grants":[],"revision":1});
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1,state='ready',runtime_state='online' WHERE workspace_id='task'").bind(&config).execute(pool).await.unwrap();
    let policy =
        json!({"githubOrgId":81,"installationId":9,"revision":"departure-policy","enabled":true});
    sqlx::query("INSERT INTO api_github_join_policies VALUES('team',9,$1,81)")
        .bind(policy)
        .execute(pool)
        .await
        .unwrap();
    for (actor, user) in [("actor-one", 42), ("actor-two", 43)] {
        sqlx::query("INSERT INTO api_github_enrollments VALUES('team',$1,9,$2)").bind(actor).bind(json!({"blocked":false,"githubUserId":user,"githubOrgId":81,"installationId":9,"revision":format!("enrollment-{actor}"),"memberId":format!("membership-{actor}")})).execute(pool).await.unwrap();
    }
    let member = access("actor-one");
    let mut admin = access("actor-two");
    admin.membership.as_mut().unwrap().role.slug = "admin".into();
    let envelope = |id: &str| commands::CommandEnvelope {
        protocol_version: commands::PROTOCOL_VERSION,
        workspace_id: "task".into(),
        session_id: id.into(),
        command_id: id.into(),
        kind: "messages.send".into(),
        fingerprint: format!("hmac-sha256:{}", "a".repeat(43)),
        schema_version: 1,
        key_version: 1,
        destruction_fence: 0,
        created_at: now(),
        iv: "fixture-iv".into(),
        ciphertext: "fixture-ciphertext".into(),
        dependencies: vec![],
        actor: None,
    };
    control
        .command_enqueue("task", &member, envelope("before-departure"), now())
        .await
        .unwrap();
    let lease_request = commands::LeaseRequest {
        storage_incarnation_id: "disk".into(),
        after_revision: None,
        wait_ms: None,
        control_only: false,
        max_items: Some(1),
    };
    let leased = control
        .command_lease("task", "runtime-fixture", &lease_request, now())
        .await
        .unwrap();
    assert_eq!(leased.leases.len(), 1);
    let historical = &leased.leases[0];
    let ack = commands::RuntimeAcknowledgment {
        command_id: historical.command.command_id.clone(),
        lease_token: historical.lease_token.clone(),
        fingerprint: historical.command.fingerprint.clone(),
        state: commands::AcknowledgmentState::Applied,
        category: Some("completed-before-departure".into()),
        result_iv: Some("retained-iv".into()),
        result_ciphertext: Some("retained-ciphertext".into()),
    };
    control
        .command_ack("task", "runtime-fixture", &ack, now())
        .await
        .unwrap();
    let retained = control
        .command_status("task", &admin, "before-departure", false, now())
        .await
        .unwrap();
    control
        .command_enqueue("task", &member, envelope("pending-departure"), now())
        .await
        .unwrap();
    control
        .command_enqueue("task", &admin, envelope("admin-survives"), now())
        .await
        .unwrap();
    let cached_checks = fixture.membership_reads.load(Ordering::SeqCst);
    let mut tickets = std::collections::HashMap::new();
    for access in [&member, &admin] {
        let ticket = control
            .gateway_ticket(
                "task",
                access,
                RUST_PROTOCOL,
                Some(RUST_RPC),
                "departure-fixture",
                now(),
            )
            .await
            .unwrap();
        tickets.insert(
            access.actor.account_id.clone(),
            ticket["credential"].as_str().unwrap().to_owned(),
        );
    }
    assert_eq!(
        fixture.membership_reads.load(Ordering::SeqCst),
        cached_checks,
        "gateway ticket requests reuse already-primed membership cache before departure"
    );
    let tickets = Arc::new(tickets);
    let hub = Arc::new(crate::gateway::Hub::default());
    let serving = control.clone();
    let serving_hub = hub.clone();
    let serving_tickets = tickets.clone();
    let app = Router::new().route(
        "/ws/{actor}",
        get(
            move |Path(actor): Path<String>, upgrade: axum::extract::WebSocketUpgrade| {
                let control = serving.clone();
                let hub = serving_hub.clone();
                let tickets = serving_tickets.clone();
                async move {
                    let Some(token) = tickets.get(&actor) else {
                        return StatusCode::UNAUTHORIZED.into_response();
                    };
                    let grant = match control
                        .gateway_authorize("task", RUST_PROTOCOL, token, now())
                        .await
                    {
                        Ok(v) => v,
                        Err(e) => return e.into_response(),
                    };
                    let room = match hub.room(control, &grant).await {
                        Ok(v) => v,
                        Err(e) => return e.into_response(),
                    };
                    let permit = hub.permit().unwrap();
                    upgrade.on_upgrade(move |socket| async move {
                        room.serve(grant, socket, permit).await
                    })
                }
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (mut member_socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{address}/ws/actor-one"))
            .await
            .unwrap();
    let (mut admin_socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{address}/ws/actor-two"))
            .await
            .unwrap();
    fixture.departed_member.store(true, Ordering::SeqCst);
    let body=serde_json::to_vec(&json!({"action":"member_removed","organization":{"id":81},"membership":{"user":{"id":42}}})).unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(b"webhook-secret").unwrap();
    mac.update(&body);
    let signature = format!(
        "sha256={}",
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let webhook = public_api
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/cloud/github/webhook")
                .header("x-hub-signature-256", signature)
                .header("x-github-delivery", "departure-gateway-queued")
                .header("x-github-event", "organization")
                .body(axum::body::Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(webhook.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        independent.membership_revision(),
        0,
        "other process never receives in-memory invalidation"
    );
    let denied = control
        .gateway_authorize("task", RUST_PROTOCOL, &tickets["actor-one"], now())
        .await;
    assert!(
        fixture.membership_reads.load(Ordering::SeqCst) > cached_checks,
        "durable invalidation forces fresh GitHub membership HTTP despite another process's cached scope"
    );
    assert!(
        denied.is_err(),
        "old gateway ticket and fresh scope cache cannot outlive durable policy invalidation"
    );
    match tokio_tungstenite::connect_async(format!("ws://{address}/ws/actor-one")).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), StatusCode::FORBIDDEN)
        }
        _ => panic!("new departed-member gateway connection must fail before upgrade"),
    }
    assert!(
        control
            .command_enqueue("task", &member, envelope("forbidden-new"), now())
            .await
            .is_err()
    );
    let delivered = control
        .command_lease("task", "runtime-fixture", &lease_request, now())
        .await
        .unwrap();
    assert_eq!(delivered.leases.len(), 1);
    assert_eq!(delivered.leases[0].command.command_id, "admin-survives");
    let rejected = control
        .command_status("task", &admin, "pending-departure", false, now())
        .await
        .unwrap();
    assert_eq!(rejected["state"], "rejected");
    assert_eq!(rejected["everLeased"], false);
    assert_eq!(rejected["category"], "organization-access-revoked");
    assert_eq!(
        control
            .command_status("task", &admin, "before-departure", false, now())
            .await
            .unwrap(),
        retained,
        "departure never rewrites an actual historical receipt"
    );
    tokio::time::timeout(Duration::from_secs(7), async {
        while let Some(frame) = member_socket.next().await {
            if let Message::Close(Some(close)) = frame.unwrap() {
                assert_eq!(u16::from(close.code), 4102);
                return;
            }
        }
        panic!("departed gateway peer ended without authorization close");
    })
    .await
    .expect("live socket must be revoked by actual gateway supervisor");
    admin_socket
        .send(Message::Ping(vec![1, 2, 3].into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(frame) = admin_socket.next().await {
            match frame.unwrap() {
                Message::Pong(bytes) => {
                    assert_eq!(&bytes[..], &[1, 2, 3]);
                    return;
                }
                Message::Close(_) => panic!("unaffected administrator was revoked"),
                _ => {}
            }
        }
        panic!("administrator disconnected");
    })
    .await
    .unwrap();
    admin_socket.close(None).await.unwrap();
    server.abort();
    fixture.departed_member.store(false, Ordering::SeqCst);
    sqlx::raw_sql("DELETE FROM api_github_enrollments; DELETE FROM api_github_join_policies")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("UPDATE api_cloud_workspaces SET request_config=$1,state='starting' WHERE workspace_id='task'").bind(original_config).execute(pool).await.unwrap();
}
