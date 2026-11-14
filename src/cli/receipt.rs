use crate::common::error::{Error, Result};
use crate::node::receipt::{ReceiptSigner, SignedReceipt};
use clap::{Args, Subcommand};
use std::fs;
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct ReceiptArgs {
    #[command(subcommand)]
    pub command: ReceiptCommands,
}

#[derive(Subcommand, Debug)]
pub enum ReceiptCommands {
    /// Cryptographically verify an Ed25519 signed inference receipt
    Verify {
        /// Path to the signed receipt JSON file
        #[arg(short, long)]
        file: PathBuf,

        /// Expected provider Ed25519 public key hex (optional; if omitted, validates self-consistency against receipt's public key)
        #[arg(short, long)]
        provider_key: Option<String>,
    },
}

impl ReceiptArgs {
    pub async fn run(&self) -> Result<()> {
        match &self.command {
            ReceiptCommands::Verify { file, provider_key } => {
                let content = fs::read_to_string(file).map_err(|e| {
                    Error::Config(format!(
                        "Failed to read receipt file '{}': {}",
                        file.display(),
                        e
                    ))
                })?;

                let receipt: SignedReceipt = serde_json::from_str(&content).map_err(|e| {
                    Error::VerificationFailed(format!("Failed to parse receipt JSON: {}", e))
                })?;

                let expected_key = provider_key.as_deref();
                ReceiptSigner::verify_signature(&receipt, expected_key)?;

                println!("Receipt Verification: SUCCESS");
                println!("  Receipt ID:      {}", receipt.payload.receipt_id);
                println!("  Request ID:      {}", receipt.payload.request_id);
                println!("  Provider ID:     {}", receipt.payload.provider_id);
                println!("  Signing Key ID:  {}", receipt.key_id);
                println!("  Model:           {}", receipt.payload.model);
                println!("  Inference State: {}", receipt.payload.inference_status);
                println!("  Payment State:   {}", receipt.payload.payment_status);
                println!(
                    "  Charge:          {} sats",
                    receipt.payload.actual_charge_sats
                );
                println!("  Algorithm:       {}", receipt.algorithm);
                if expected_key.is_some() {
                    println!("  Provider Trust:  VERIFIED against trusted public key");
                } else {
                    println!("  Provider Trust:  SELF-CONSISTENT (unpinned public key)");
                }

                Ok(())
            }
        }
    }
}
