//! SatsGrid: Permissionless Open-Model Inference Paid in Sats via L402.

pub mod cli;
pub mod client;
pub mod common;
pub mod config;
pub mod discovery;
pub mod node;

pub use common::error::{Error, Result};
