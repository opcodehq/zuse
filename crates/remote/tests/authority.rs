#![cfg(not(target_arch = "wasm32"))]
use zuse_remote::{
    PairId,
    authority::{Authority, Error},
};
#[test]
fn grants_persist_without_metadata_disclosure_and_revocation_cancels_connections() {
    let root = tempfile::tempdir().unwrap();
    let authority = Authority::open(root.path()).unwrap();
    assert!(matches!(Authority::open(root.path()), Err(Error::Busy)));
    let (metadata, invitation) = authority.create("My laptop").unwrap();
    assert!(metadata.paired_at.is_none() && metadata.expires_at.is_some());
    let credentials = authority.credentials(&invitation.id).unwrap();
    let list = serde_json::to_string(&authority.list().unwrap()).unwrap();
    assert!(!list.contains(&invitation.secret.expose()));
    assert!(authority.list().unwrap()[0].paired_at.is_none());
    let paired = authority.confirm(&credentials).unwrap();
    assert!(paired.paired_at.is_some() && paired.expires_at.is_none());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(root.path().join("pairing"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(root.path().join("pairing/peers.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    authority.close();
    assert!(!credentials.is_live());
    let reopened = Authority::open(root.path()).unwrap();
    assert!(reopened.list().unwrap()[0].paired_at.is_some());
    let connection = reopened.credentials(&invitation.id).unwrap();
    assert!(reopened.confirm(&credentials).is_err());
    reopened.revoke(&invitation.id).unwrap();
    assert!(!connection.is_live());
    assert!(reopened.credentials(&invitation.id).is_err());
    reopened.close();
    assert!(
        Authority::open(root.path())
            .unwrap()
            .list()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn expired_invitations_and_other_authorities_cannot_confirm_a_grant() {
    let root = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let authority = Authority::open(root.path()).unwrap();
    let (_, invitation) = authority.create("Phone").unwrap();
    let credentials = authority.credentials(&invitation.id).unwrap();
    let different = Authority::open(other.path()).unwrap();
    assert!(different.confirm(&credentials).is_err());
    authority.close();
    let path = root.path().join("pairing/peers.json");
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    stored["peers"][0]["metadata"]["expiresAt"] = serde_json::json!(1);
    std::fs::write(&path, serde_json::to_vec(&stored).unwrap()).unwrap();
    let reopened = Authority::open(root.path()).unwrap();
    assert!(matches!(
        reopened.credentials(&invitation.id),
        Err(Error::Expired)
    ));
}
#[test]
fn damaged_pairing_storage_is_preserved_instead_of_reset() {
    let root = tempfile::tempdir().unwrap();
    let authority = Authority::open(root.path()).unwrap();
    authority.create("Phone").unwrap();
    authority.close();
    let path = root.path().join("pairing/peers.json");
    std::fs::write(&path, b"damaged saved data").unwrap();
    assert!(matches!(Authority::open(root.path()), Err(Error::Invalid)));
    assert_eq!(std::fs::read(path).unwrap(), b"damaged saved data");
}
#[test]
fn failed_confirmation_never_authorizes_rpc_and_closes_existing_leases() {
    let root = tempfile::tempdir().unwrap();
    let authority = Authority::open(root.path()).unwrap();
    let (_, invitation) = authority.create("Phone").unwrap();
    let credentials = authority.credentials(&invitation.id).unwrap();
    let path = root.path().join("pairing/peers.json");
    let backup = root.path().join("pairing/peers.backup");
    std::fs::rename(&path, &backup).unwrap();
    std::fs::create_dir(&path).unwrap();
    let failed = authority.confirm(&credentials);
    std::fs::remove_dir(&path).unwrap();
    std::fs::rename(&backup, &path).unwrap();
    assert!(matches!(failed, Err(Error::Storage)));
    assert!(!credentials.is_live());
    assert!(matches!(authority.list(), Err(Error::Closed)));
    let reopened = Authority::open(root.path()).unwrap();
    assert!(reopened.list().unwrap()[0].paired_at.is_none());
}
#[test]
fn invitation_count_and_device_labels_are_bounded() {
    let root = tempfile::tempdir().unwrap();
    let authority = Authority::open(root.path()).unwrap();
    for label in ["", "\n", "a\nb"] {
        assert!(matches!(authority.create(label), Err(Error::Label)));
    }
    for index in 0..32 {
        authority.create(&format!("Device {index}")).unwrap();
    }
    assert!(matches!(authority.create("One too many"), Err(Error::Full)));
    let first = PairId::parse(&authority.list().unwrap()[0].id).unwrap();
    authority.revoke(&first).unwrap();
    authority.create("Replacement").unwrap();
}
