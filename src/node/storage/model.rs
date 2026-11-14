use crate::common::error::{Error, Result};
use sqlx::{Pool, Sqlite};

/// Durable record of a registered AI model in the node's catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRecord {
    pub id: String,
    pub name: String,
    pub owned_by: String,
    pub description: Option<String>,
    pub context_length: Option<i64>,
    pub flat_rate_sats: Option<u64>,
    pub sats_per_prompt_token: Option<u64>,
    pub sats_per_completion_token: Option<u64>,
    pub is_enabled: bool,
    pub is_available: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Metadata payload discovered from an upstream inference engine (e.g. Ollama/vLLM).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredModel {
    pub id: String,
    pub name: String,
    pub owned_by: Option<String>,
    pub description: Option<String>,
    pub context_length: Option<i64>,
}

type ModelRow = (
    String,
    String,
    String,
    Option<String>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    Option<i64>,
    bool,
    bool,
    i64,
    i64,
);

fn map_model_row(r: ModelRow) -> ModelRecord {
    ModelRecord {
        id: r.0,
        name: r.1,
        owned_by: r.2,
        description: r.3,
        context_length: r.4,
        flat_rate_sats: r.5.map(|s| s.max(0) as u64),
        sats_per_prompt_token: r.6.map(|s| s.max(0) as u64),
        sats_per_completion_token: r.7.map(|s| s.max(0) as u64),
        is_enabled: r.8,
        is_available: r.9,
        created_at: r.10,
        updated_at: r.11,
    }
}

/// Store for managing model catalog metadata, upstream sync, and operator pricing overrides.
#[derive(Clone, Debug)]
pub struct ModelStore {
    pool: Pool<Sqlite>,
}

impl ModelStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    /// Upsert models discovered from a successful upstream inventory scan.
    ///
    /// Preserves operator overrides:
    /// - `is_enabled`
    /// - `flat_rate_sats`
    /// - `sats_per_prompt_token`
    /// - `sats_per_completion_token`
    ///
    /// Lifecycle rules:
    /// - Newly discovered models are created as enabled (`is_enabled = 1`) and available (`is_available = 1`).
    /// - Existing models reported in `discovered` are updated with fresh name, description, context_length, and marked `is_available = 1`.
    /// - If `mark_missing_unavailable` is true, models previously in the database that are NOT in `discovered` are marked `is_available = 0`.
    ///   Models are NEVER deleted, preserving historical references for receipts and requests.
    pub async fn upsert_discovered(
        &self,
        discovered: &[DiscoveredModel],
        mark_missing_unavailable: bool,
    ) -> Result<Vec<ModelRecord>> {
        let now = chrono::Utc::now().timestamp();
        let mut tx = self.pool.begin().await.map_err(|e| {
            Error::Internal(format!(
                "Failed to begin transaction for model upsert: {}",
                e
            ))
        })?;

        let mut seen_ids = Vec::with_capacity(discovered.len());

        for model in discovered {
            seen_ids.push(model.id.clone());
            let owned_by = model.owned_by.as_deref().unwrap_or("satsgrid-node");

            let query = r#"
                INSERT INTO models (
                    id, name, owned_by, description, context_length,
                    is_enabled, is_available, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, 1, 1, ?6, ?6)
                ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    owned_by = excluded.owned_by,
                    description = COALESCE(excluded.description, models.description),
                    context_length = COALESCE(excluded.context_length, models.context_length),
                    is_available = 1,
                    updated_at = excluded.updated_at
            "#;

            sqlx::query(query)
                .bind(&model.id)
                .bind(&model.name)
                .bind(owned_by)
                .bind(&model.description)
                .bind(model.context_length)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    Error::Internal(format!(
                        "Failed to upsert discovered model '{}': {}",
                        model.id, e
                    ))
                })?;
        }

        if mark_missing_unavailable && !seen_ids.is_empty() {
            // Update models that were NOT in the seen_ids list
            // We can fetch existing IDs and update them cleanly with static queries
            let existing_ids: Vec<String> =
                sqlx::query_scalar("SELECT id FROM models WHERE is_available = 1")
                    .fetch_all(&mut *tx)
                    .await
                    .map_err(|e| {
                        Error::Internal(format!(
                            "Failed to query models for availability update: {}",
                            e
                        ))
                    })?;

            for id in existing_ids {
                if !seen_ids.contains(&id) {
                    sqlx::query(
                        "UPDATE models SET is_available = 0, updated_at = ?1 WHERE id = ?2",
                    )
                    .bind(now)
                    .bind(&id)
                    .execute(&mut *tx)
                    .await
                    .map_err(|e| {
                        Error::Internal(format!("Failed to mark model '{}' unavailable: {}", id, e))
                    })?;
                }
            }
        }

        tx.commit().await.map_err(|e| {
            Error::Internal(format!("Failed to commit model upsert transaction: {}", e))
        })?;

        self.list_models(None, None).await
    }

    /// Retrieve a single model record by ID.
    pub async fn get_model(&self, id: &str) -> Result<Option<ModelRecord>> {
        let query = r#"
            SELECT id, name, owned_by, description, context_length,
                   flat_rate_sats, sats_per_prompt_token, sats_per_completion_token,
                   is_enabled, is_available, created_at, updated_at
            FROM models
            WHERE id = ?1
        "#;

        let row: Option<ModelRow> = sqlx::query_as(query)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch model '{}': {}", id, e)))?;

        Ok(row.map(map_model_row))
    }

    /// List models with optional filtering by enabled and available status.
    pub async fn list_models(
        &self,
        enabled_only: Option<bool>,
        available_only: Option<bool>,
    ) -> Result<Vec<ModelRecord>> {
        let query = match (enabled_only, available_only) {
            (Some(true), Some(true)) => {
                r#"
                SELECT id, name, owned_by, description, context_length,
                       flat_rate_sats, sats_per_prompt_token, sats_per_completion_token,
                       is_enabled, is_available, created_at, updated_at
                FROM models
                WHERE is_enabled = 1 AND is_available = 1
                ORDER BY name ASC, id ASC
                "#
            }
            (Some(true), Some(false)) => {
                r#"
                SELECT id, name, owned_by, description, context_length,
                       flat_rate_sats, sats_per_prompt_token, sats_per_completion_token,
                       is_enabled, is_available, created_at, updated_at
                FROM models
                WHERE is_enabled = 1 AND is_available = 0
                ORDER BY name ASC, id ASC
                "#
            }
            (Some(false), Some(true)) => {
                r#"
                SELECT id, name, owned_by, description, context_length,
                       flat_rate_sats, sats_per_prompt_token, sats_per_completion_token,
                       is_enabled, is_available, created_at, updated_at
                FROM models
                WHERE is_enabled = 0 AND is_available = 1
                ORDER BY name ASC, id ASC
                "#
            }
            (Some(false), Some(false)) => {
                r#"
                SELECT id, name, owned_by, description, context_length,
                       flat_rate_sats, sats_per_prompt_token, sats_per_completion_token,
                       is_enabled, is_available, created_at, updated_at
                FROM models
                WHERE is_enabled = 0 AND is_available = 0
                ORDER BY name ASC, id ASC
                "#
            }
            (Some(true), None) => {
                r#"
                SELECT id, name, owned_by, description, context_length,
                       flat_rate_sats, sats_per_prompt_token, sats_per_completion_token,
                       is_enabled, is_available, created_at, updated_at
                FROM models
                WHERE is_enabled = 1
                ORDER BY name ASC, id ASC
                "#
            }
            (Some(false), None) => {
                r#"
                SELECT id, name, owned_by, description, context_length,
                       flat_rate_sats, sats_per_prompt_token, sats_per_completion_token,
                       is_enabled, is_available, created_at, updated_at
                FROM models
                WHERE is_enabled = 0
                ORDER BY name ASC, id ASC
                "#
            }
            (None, Some(true)) => {
                r#"
                SELECT id, name, owned_by, description, context_length,
                       flat_rate_sats, sats_per_prompt_token, sats_per_completion_token,
                       is_enabled, is_available, created_at, updated_at
                FROM models
                WHERE is_available = 1
                ORDER BY name ASC, id ASC
                "#
            }
            (None, Some(false)) => {
                r#"
                SELECT id, name, owned_by, description, context_length,
                       flat_rate_sats, sats_per_prompt_token, sats_per_completion_token,
                       is_enabled, is_available, created_at, updated_at
                FROM models
                WHERE is_available = 0
                ORDER BY name ASC, id ASC
                "#
            }
            (None, None) => {
                r#"
                SELECT id, name, owned_by, description, context_length,
                       flat_rate_sats, sats_per_prompt_token, sats_per_completion_token,
                       is_enabled, is_available, created_at, updated_at
                FROM models
                ORDER BY name ASC, id ASC
                "#
            }
        };

        let rows: Vec<ModelRow> = sqlx::query_as(query)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to list models: {}", e)))?;

        Ok(rows.into_iter().map(map_model_row).collect())
    }

    /// Operator action: enable or disable a model.
    pub async fn set_enabled(&self, id: &str, is_enabled: bool) -> Result<bool> {
        let now = chrono::Utc::now().timestamp();
        let query = "UPDATE models SET is_enabled = ?1, updated_at = ?2 WHERE id = ?3";

        let result = sqlx::query(query)
            .bind(is_enabled)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to set model enabled state: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }

    /// Operator action: set model-specific pricing overrides.
    /// Pass `None` to revert a field to node defaults.
    pub async fn set_pricing(
        &self,
        id: &str,
        flat_rate_sats: Option<u64>,
        sats_per_prompt_token: Option<u64>,
        sats_per_completion_token: Option<u64>,
    ) -> Result<bool> {
        let now = chrono::Utc::now().timestamp();
        let flat_signed = flat_rate_sats.map(|v| v as i64);
        let prompt_signed = sats_per_prompt_token.map(|v| v as i64);
        let completion_signed = sats_per_completion_token.map(|v| v as i64);

        let query = r#"
            UPDATE models
            SET flat_rate_sats = ?1,
                sats_per_prompt_token = ?2,
                sats_per_completion_token = ?3,
                updated_at = ?4
            WHERE id = ?5
        "#;

        let result = sqlx::query(query)
            .bind(flat_signed)
            .bind(prompt_signed)
            .bind(completion_signed)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to set model pricing: {}", e)))?;

        Ok(result.rows_affected() > 0)
    }
}
