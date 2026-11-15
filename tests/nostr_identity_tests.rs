use nostr_sdk::prelude::*;
use satsgrid::discovery::announcement::NodeAnnouncement;
use satsgrid::node::server::SatsGridServer;
use tempfile::tempdir;

#[test]
fn test_nostr_identity_persistence_across_restarts() {
    let tmp = tempdir().expect("create tempdir");
    let data_dir = tmp.path().to_str().unwrap();

    // 1. Initial generation
    let keys1 = SatsGridServer::load_or_generate_nostr_identity(data_dir, None)
        .expect("generate initial keys");
    let pubkey1 = keys1.public_key().to_hex();

    // 2. Restart simulation: reloading from data_dir must yield identical keys
    let keys2 = SatsGridServer::load_or_generate_nostr_identity(data_dir, None)
        .expect("reload stored keys");
    let pubkey2 = keys2.public_key().to_hex();

    assert_eq!(pubkey1, pubkey2);

    // 3. Explicit configured nsec override takes precedence
    let custom_keys = nostr_sdk::prelude::Keys::generate();
    let custom_nsec = custom_keys.secret_key().to_secret_hex();
    let keys3 = SatsGridServer::load_or_generate_nostr_identity(data_dir, Some(&custom_nsec))
        .expect("use explicit override");
    assert_eq!(keys3.public_key(), custom_keys.public_key());
}

#[test]
fn test_announcement_with_receipt_key_id_roundtrip() {
    let keys = nostr_sdk::prelude::Keys::generate();
    let announcement = NodeAnnouncement::new(
        "Provider Alpha",
        "https://node.satsgrid.net:8080",
        vec!["llama3.2".to_string()],
        15,
        "regtest",
    )
    .with_receipt_key_id("ed25519:abcdef123456");

    let builder = announcement.to_event_builder().expect("build event");
    let event = builder.finalize(&keys).expect("sign event");

    let parsed = NodeAnnouncement::from_event(&event).expect("parse and verify event");
    assert_eq!(parsed.name, "Provider Alpha");
    assert_eq!(
        parsed.receipt_key_id,
        Some("ed25519:abcdef123456".to_string())
    );
    assert_eq!(parsed.pubkey, Some(keys.public_key().to_hex()));
}
