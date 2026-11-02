use zuse_remote::{Invitation, PairedTarget};
#[test]
fn paired_targets_roundtrip_private_links_without_exposing_keys_in_socket_urls_or_debug() {
    for origin in [
        "https://engine.example:8443",
        "http://127.0.0.1:27666",
        "http://[::1]:27666",
    ] {
        let target = PairedTarget::new(origin, Invitation::generate().unwrap()).unwrap();
        let exported = target.export_link().unwrap();
        let restored = PairedTarget::parse_link(&exported).unwrap();
        assert_eq!(restored.socket_url(), target.socket_url());
        assert!(
            target
                .socket_url()
                .ends_with(target.invitation().id.as_str())
        );
        assert!(
            !target
                .socket_url()
                .contains(&target.invitation().secret.expose())
        );
        assert!(!format!("{target:?}").contains(&target.invitation().secret.expose()));
        assert!(
            target
                .socket_url()
                .starts_with(if origin.starts_with("https") {
                    "wss://"
                } else {
                    "ws://"
                })
        );
    }
}
#[test]
fn paired_targets_reject_credentials_queries_paths_and_insecure_remote_app_origins() {
    for origin in [
        "https://user:password@engine.example",
        "https://engine.example/api/rpc",
        "https://engine.example?token=private",
        "https://engine.example#private",
        "http://engine.example",
        "ws://192.168.1.2",
        "file:///tmp/engine",
        "javascript:alert(1)",
    ] {
        assert!(
            PairedTarget::new(origin, Invitation::generate().unwrap()).is_err(),
            "accepted {origin}"
        );
    }
    assert!(PairedTarget::parse_link("https://engine.example/#token=wrong").is_err());
    assert!(PairedTarget::parse_link(&"a".repeat(2049)).is_err());
}
#[test]
fn native_private_export_import_is_bounded_and_reusable() {
    let root = tempfile::tempdir().unwrap();
    let target =
        PairedTarget::new("https://engine.example", Invitation::generate().unwrap()).unwrap();
    let path = root.path().join("paired-link.txt");
    std::fs::write(&path, target.export_link().unwrap()).unwrap();
    let imported = PairedTarget::read_link_file(&path).unwrap();
    assert_eq!(imported.socket_url(), target.socket_url());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    std::fs::write(&path, "x".repeat(2049)).unwrap();
    assert!(PairedTarget::read_link_file(&path).is_err());
    assert!(PairedTarget::read_link_file(root.path()).is_err());
}
