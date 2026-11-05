use crate::node::storage::Database;
use std::sync::Arc;

/// Helper to create a shared in-memory Database instance for test AppState harnesses.
pub async fn test_database() -> Arc<Database> {
    let db = Database::init_in_memory()
        .await
        .expect("in-memory database should initialize");
    Arc::new(db)
}
