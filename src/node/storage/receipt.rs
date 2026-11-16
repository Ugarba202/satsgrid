use crate::common::error::{Error, Result};
use crate::node::receipt::SignedReceipt;
use sqlx::{Pool, Sqlite};

/// Durable store for immutable PayGuard cryptographically signed receipts.
#[derive(Clone, Debug)]
pub struct ReceiptStore {
    pool: Pool<Sqlite>,
}

impl ReceiptStore {
    pub fn new(pool: Pool<Sqlite>) -> Self {
        Self { pool }
    }

    /// Persist a newly signed receipt immutably.
    pub async fn create_receipt(&self, receipt: &SignedReceipt) -> Result<()> {
        let canonical_payload = receipt.payload.to_canonical_json()?;
        let query = r#"
            INSERT INTO receipts (
                id, request_id, receipt_version, signing_key_id, signature_algorithm,
                public_key_hex, canonical_payload, signature_hex, inference_status,
                payment_status, charged_sats, supersedes_receipt_id, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
        "#;

        sqlx::query(query)
            .bind(&receipt.payload.receipt_id)
            .bind(&receipt.payload.request_id)
            .bind(receipt.payload.schema_version as i64)
            .bind(&receipt.key_id)
            .bind(&receipt.algorithm)
            .bind(&receipt.public_key_hex)
            .bind(&canonical_payload)
            .bind(&receipt.signature_hex)
            .bind(&receipt.payload.inference_status)
            .bind(&receipt.payload.payment_status)
            .bind(receipt.payload.actual_charge_sats as i64)
            .bind(receipt.payload.supersedes_receipt_id.as_deref())
            .bind(receipt.payload.created_at)
            .execute(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(ref db_err) if db_err.is_unique_violation() => {
                    Error::Internal(format!(
                        "Receipt '{}' already exists: {}",
                        receipt.payload.receipt_id, db_err
                    ))
                }
                _ => Error::Internal(format!("Failed to insert receipt: {}", e)),
            })?;

        Ok(())
    }

    /// Fetch a signed receipt by its unique receipt ID.
    pub async fn get_receipt(&self, receipt_id: &str) -> Result<Option<SignedReceipt>> {
        let query = r#"
            SELECT canonical_payload, signature_hex, public_key_hex, signature_algorithm, signing_key_id
            FROM receipts
            WHERE id = ?1
        "#;

        let row: Option<(String, String, String, String, String)> = sqlx::query_as(query)
            .bind(receipt_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| Error::Internal(format!("Failed to fetch receipt: {}", e)))?;

        match row {
            Some((payload_json, sig_hex, pub_hex, algo, key_id)) => {
                let payload = serde_json::from_str(&payload_json).map_err(|e| {
                    Error::Internal(format!("Corrupted receipt payload JSON: {}", e))
                })?;
                Ok(Some(SignedReceipt {
                    payload,
                    signature_hex: sig_hex,
                    public_key_hex: pub_hex,
                    algorithm: algo,
                    key_id,
                }))
            }
            None => Ok(None),
        }
    }

    /// Fetch latest receipt for a given request ID.
    pub async fn get_receipt_by_request_id(
        &self,
        request_id: &str,
    ) -> Result<Option<SignedReceipt>> {
        let query = r#"
            SELECT canonical_payload, signature_hex, public_key_hex, signature_algorithm, signing_key_id
            FROM receipts
            WHERE request_id = ?1
            ORDER BY created_at DESC LIMIT 1
        "#;

        let row: Option<(String, String, String, String, String)> = sqlx::query_as(query)
            .bind(request_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| {
                Error::Internal(format!("Failed to fetch receipt by request ID: {}", e))
            })?;

        match row {
            Some((payload_json, sig_hex, pub_hex, algo, key_id)) => {
                let payload = serde_json::from_str(&payload_json).map_err(|e| {
                    Error::Internal(format!("Corrupted receipt payload JSON: {}", e))
                })?;
                Ok(Some(SignedReceipt {
                    payload,
                    signature_hex: sig_hex,
                    public_key_hex: pub_hex,
                    algorithm: algo,
                    key_id,
                }))
            }
            None => Ok(None),
        }
    }
}
