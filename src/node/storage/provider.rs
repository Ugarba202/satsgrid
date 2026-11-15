use crate::common::error::{Error, Result};
use crate::discovery::announcement::NodeAnnouncement;
use sqlx::{Pool, Sqlite};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRecord {
    pub pubkey: String,
    pub name: String,
    pub endpoint: String,
    pub models: Vec<String>,
    pub pricing_sats: u64,
    pub network: String,
    pub version: String,
    pub receipt_key_id: Option<String>,
    pub status: String,
    pub last_seen_at: i64,
    pub last_checked_at: Option<i64>,
    pub last_error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

type ProviderRow = (
    String,
    String,
    String,
    String,
    i64,
    String,
    String,
    Option<String>,
    String,
    i64,
    Option<i64>,
    Option<String>,
    i64,
    i64,
);

fn map_provider_row(r: ProviderRow) -> ProviderRecord {
    let models: Vec<String> = serde_json::from_str(&r.3).unwrap_or_default();
    ProviderRecord {
        pubkey: r.0,
        name: r.1,
        endpoint: r.2,
        models,
        pricing_sats: r.4.max(0) as u64,
        network: r.5,
        version: r.6,
        receipt_key_id: r.7,
        status: r.8,
        last_seen_at: r.9,
        last_checked_at: r.10,
        last_error: r.11,
        created_at: r.12,
        updated_at: r.13,
    }
}

#[derive(Clone, Debug)]
pub struct ProviderStore {
    pool: Pool<Sqlite>,
}

impl ProviderStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    /// Upsert a verified discovered node announcement into the persistent registry.
    pub async fn upsert_announcement(&self, ann: &NodeAnnouncement) -> Result<ProviderRecord> {
        let pubkey = ann.pubkey.clone().unwrap_or_else(|| ann.node_url.clone());
        let models_json = serde_json::to_string(&ann.models).unwrap_or_else(|_| "[]".to_string());
        let now = chrono::Utc::now().timestamp();

        let query = r#"
            INSERT INTO providers (
                pubkey, name, endpoint, models, pricing_sats, network, version,
                receipt_key_id, status, last_seen_at, created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'discovered', ?9, ?9, ?9)
            ON CONFLICT(pubkey) DO UPDATE SET
                name = excluded.name,
                endpoint = excluded.endpoint,
                models = excluded.models,
                pricing_sats = excluded.pricing_sats,
                network = excluded.network,
                version = excluded.version,
                receipt_key_id = COALESCE(excluded.receipt_key_id, providers.receipt_key_id),
                last_seen_at = excluded.last_seen_at,
                updated_at = excluded.updated_at
        "#;

        sqlx::query(query)
            .bind(&pubkey)
            .bind(&ann.name)
            .bind(&ann.node_url)
            .bind(&models_json)
            .bind(ann.pricing_sats as i64)
            .bind(&ann.network)
            .bind(&ann.version)
            .bind(&ann.receipt_key_id)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to upsert provider '{}': {}", pubkey, e))
            })?;

        self.get_provider(&pubkey)
            .await?
            .ok_or_else(|| Error::Internal("Provider record missing after upsert".to_string()))
    }

    /// Update health check result for a provider.
    pub async fn update_health(
        &self,
        pubkey: &str,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<()> {
        let now = chrono::Utc::now().timestamp();
        let query = r#"
            UPDATE providers
            SET status = ?1, last_checked_at = ?2, last_error = ?3, updated_at = ?2
            WHERE pubkey = ?4
        "#;

        sqlx::query(query)
            .bind(status)
            .bind(now)
            .bind(error_message)
            .bind(pubkey)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                Error::Internal(format!(
                    "Failed to update provider health '{}': {}",
                    pubkey, e
                ))
            })?;

        Ok(())
    }

    /// Get a single provider by pubkey.
    pub async fn get_provider(&self, pubkey: &str) -> Result<Option<ProviderRecord>> {
        let query = r#"
            SELECT pubkey, name, endpoint, models, pricing_sats, network, version,
                   receipt_key_id, status, last_seen_at, last_checked_at, last_error,
                   created_at, updated_at
            FROM providers
            WHERE pubkey = ?1
        "#;

        let row: Option<ProviderRow> = sqlx::query_as(query)
            .bind(pubkey)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to fetch provider '{}': {}", pubkey, e))
            })?;

        Ok(row.map(map_provider_row))
    }

    /// List providers with optional status filtering.
    pub async fn list_providers(&self, status_filter: Option<&str>) -> Result<Vec<ProviderRecord>> {
        let now = chrono::Utc::now().timestamp();
        const STALE_PROVIDER_AGE_SECS: i64 = 86400; // 24 hours

        let rows: Vec<ProviderRow> = if let Some(status) = status_filter {
            sqlx::query_as(
                r#"
                SELECT pubkey, name, endpoint, models, pricing_sats, network, version,
                       receipt_key_id, status, last_seen_at, last_checked_at, last_error,
                       created_at, updated_at
                FROM providers
                WHERE status = ?1
                ORDER BY last_seen_at DESC
                "#,
            )
            .bind(status)
            .fetch_all(&self.pool)
            .await
        } else {
            sqlx::query_as(
                r#"
                SELECT pubkey, name, endpoint, models, pricing_sats, network, version,
                       receipt_key_id, status, last_seen_at, last_checked_at, last_error,
                       created_at, updated_at
                FROM providers
                ORDER BY last_seen_at DESC
                "#,
            )
            .fetch_all(&self.pool)
            .await
        }
        .map_err(|e| Error::Internal(format!("Failed to list providers: {}", e)))?;

        let providers = rows
            .into_iter()
            .map(map_provider_row)
            .map(|mut p| {
                // If the provider has not been seen for over 24 hours and is still marked discovered/healthy, mark it stale
                if (now - p.last_seen_at) > STALE_PROVIDER_AGE_SECS
                    && (p.status == "discovered" || p.status == "healthy")
                {
                    p.status = "stale".to_string();
                }
                p
            })
            .collect();

        Ok(providers)
    }

    /// Find eligible providers that serve a specific model.
    pub async fn find_providers_for_model(&self, model: &str) -> Result<Vec<ProviderRecord>> {
        let all = self.list_providers(None).await?;
        let now = chrono::Utc::now().timestamp();
        const MAX_PROVIDER_AGE_SECS: i64 = 86400; // 24 hours

        let matching = all
            .into_iter()
            .filter(|p| {
                // Must serve model, not be offline/error, and announcement must not be expired
                let is_fresh = (now - p.last_seen_at) <= MAX_PROVIDER_AGE_SECS;
                p.models.iter().any(|m| m == model)
                    && p.status != "offline"
                    && p.status != "error"
                    && is_fresh
            })
            .collect();
        Ok(matching)
    }
}
