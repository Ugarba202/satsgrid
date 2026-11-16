use satsgrid::discovery::announcement::NodeAnnouncement;
use satsgrid::node::storage::Database;
use tempfile::tempdir;

#[tokio::test]
async fn test_provider_store_upsert_and_health_tracking() {
    let tmp = tempdir().expect("create tempdir");
    let db = Database::init(tmp.path()).await.expect("init db");
    let provider_store = db.provider_store();

    // 1. Initial list empty
    let initial = provider_store.list_providers(None).await.expect("list");
    assert!(initial.is_empty());

    // 2. Upsert announcement
    let ann = NodeAnnouncement::new(
        "Compute Node 1",
        "https://node1.satsgrid.net:8080",
        vec!["llama3.2".to_string(), "mistral-7b".to_string()],
        15,
        "regtest",
    )
    .with_receipt_key_id("ed25519:testkey123");

    let mut ann_with_key = ann.clone();
    ann_with_key.pubkey = Some("02abcdef0102030405060708090a0b0c0d0e0f".to_string());

    let record = provider_store
        .upsert_announcement(&ann_with_key)
        .await
        .expect("upsert provider");
    assert_eq!(record.name, "Compute Node 1");
    assert_eq!(record.status, "discovered");
    assert_eq!(record.pricing_sats, 15);
    assert_eq!(
        record.receipt_key_id,
        Some("ed25519:testkey123".to_string())
    );

    // 3. Update health status
    provider_store
        .update_health(&record.pubkey, "healthy", None)
        .await
        .expect("update health");

    let updated = provider_store
        .get_provider(&record.pubkey)
        .await
        .expect("get provider")
        .expect("provider exists");
    assert_eq!(updated.status, "healthy");

    // 4. Find providers for model
    let llama_providers = provider_store
        .find_providers_for_model("llama3.2")
        .await
        .expect("find providers");
    assert_eq!(llama_providers.len(), 1);
    assert_eq!(llama_providers[0].pubkey, record.pubkey);

    let unknown_providers = provider_store
        .find_providers_for_model("gpt-4")
        .await
        .expect("find providers");
    assert!(unknown_providers.is_empty());
}
