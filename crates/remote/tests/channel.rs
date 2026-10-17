use zuse_remote::*;
fn channels() -> (SecureChannel, SecureChannel) {
    let invitation = Invitation::generate().unwrap();
    let (client, first) = initiate(&invitation.id, &invitation.secret).unwrap();
    let (host, reply) = accept(&invitation.id, &invitation.secret, &first).unwrap();
    (client.finish(&reply).unwrap(), host)
}
#[test]
fn private_invitation_roundtrips_without_debug_disclosure() {
    let invitation = Invitation::generate().unwrap();
    let encoded = invitation.fragment().unwrap();
    let decoded = Invitation::parse_fragment(&encoded).unwrap();
    assert_eq!(decoded.id, invitation.id);
    assert_eq!(decoded.secret.expose(), invitation.secret.expose());
    assert!(!format!("{invitation:?}").contains(&invitation.secret.expose()));
    for invalid in [
        "",
        "#pair=",
        "?pair=value",
        "#pair=garbage",
        "#pair=eyJ2IjoyfQ",
    ] {
        assert!(Invitation::parse_fragment(invalid).is_err());
    }
}
#[test]
fn handshake_authenticates_both_key_and_pair_identity() {
    let invitation = Invitation::generate().unwrap();
    let (_, first) = initiate(&invitation.id, &invitation.secret).unwrap();
    let wrong = Invitation::generate().unwrap();
    assert!(accept(&invitation.id, &wrong.secret, &first).is_err());
    assert!(accept(&wrong.id, &invitation.secret, &first).is_err());
    let (client, first) = initiate(&invitation.id, &invitation.secret).unwrap();
    let (_, mut reply) = accept(&invitation.id, &invitation.secret, &first).unwrap();
    let last = reply.len() - 1;
    reply[last] ^= 1;
    assert!(client.finish(&reply).is_err());
}
#[test]
fn large_bidirectional_messages_are_bounded_authenticated_records() {
    let (mut client, mut host) = channels();
    for size in [0, 1, 65_514, 65_515, MAX_MESSAGE_BYTES] {
        let message = (0..size).map(|i| (i % 251) as u8).collect::<Vec<_>>();
        let records = client.seal(&message).unwrap();
        assert!(records.iter().all(|r| r.len() <= MAX_RECORD_BYTES));
        let mut received = None;
        for record in &records {
            let next = host.open(record).unwrap();
            assert!(received.is_none());
            received = next;
        }
        assert_eq!(received.unwrap(), message);
        let reply = b"reply to accepted request";
        let record = host.seal(reply).unwrap();
        assert_eq!(client.open(&record[0]).unwrap().unwrap(), reply);
    }
    assert!(matches!(
        client.seal(&vec![0; MAX_MESSAGE_BYTES + 1]),
        Err(Error::Limit)
    ));
    assert!(!client.is_closed());
}
#[test]
fn replay_tampering_reordering_and_reflection_close_the_session() {
    for attack in 0..4 {
        let (mut client, mut host) = channels();
        let records = client.seal(&vec![7; 100_000]).unwrap();
        match attack {
            0 => {
                host.open(&records[0]).unwrap();
                assert!(host.open(&records[0]).is_err());
            }
            1 => {
                let mut record = records[0].clone();
                record[20] ^= 1;
                assert!(host.open(&record).is_err());
            }
            2 => {
                assert!(host.open(&records[1]).is_err());
            }
            _ => {
                let reply = host.seal(b"reflection").unwrap();
                assert!(host.open(&reply[0]).is_err());
            }
        }
        assert!(host.is_closed());
        assert!(matches!(host.open(&records[0]), Err(Error::Closed)));
        assert!(matches!(
            host.seal(b"no further traffic"),
            Err(Error::Closed)
        ));
    }
}
#[test]
fn reconnect_uses_fresh_keys_and_cannot_accept_previous_ciphertext() {
    let invitation = Invitation::generate().unwrap();
    let connect = || {
        let (client, first) = initiate(&invitation.id, &invitation.secret).unwrap();
        let (host, reply) = accept(&invitation.id, &invitation.secret, &first).unwrap();
        (client.finish(&reply).unwrap(), host)
    };
    let (mut first, _) = connect();
    let old = first.seal(b"old command").unwrap();
    let (mut second, mut host) = connect();
    let fresh = second.seal(b"old command").unwrap();
    assert_ne!(old, fresh);
    assert!(host.open(&old[0]).is_err());
}
