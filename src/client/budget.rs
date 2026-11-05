use crate::client::error::ClientError;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug)]
pub struct ClientBudgetTracker {
    allocated_sats: u64,
    spent_sats: AtomicU64,
}

impl ClientBudgetTracker {
    pub fn new(allocated_sats: u64) -> Self {
        Self {
            allocated_sats,
            spent_sats: AtomicU64::new(0),
        }
    }

    /// Spends the requested amount atomically using a CAS (compare-and-swap) loop.
    /// Safely handles race conditions without check-then-act vulnerabilities.
    pub fn spend(&self, amount_sats: u64) -> Result<(), ClientError> {
        let mut current = self.spent_sats.load(Ordering::SeqCst);

        loop {
            // Defensive arithmetic to prevent u64 overflow panics
            let next =
                current
                    .checked_add(amount_sats)
                    .ok_or_else(|| ClientError::BudgetExceeded {
                        allocated: self.allocated_sats,
                        needed: amount_sats,
                    })?;

            if next > self.allocated_sats {
                return Err(ClientError::BudgetExceeded {
                    allocated: self.allocated_sats,
                    needed: amount_sats,
                });
            }

            match self.spent_sats.compare_exchange_weak(
                current,
                next,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => {
                    // Another thread updated the spent amount, update our view and retry.
                    current = actual;
                }
            }
        }
    }

    pub fn remaining(&self) -> u64 {
        let spent = self.spent_sats.load(Ordering::SeqCst);
        self.allocated_sats.saturating_sub(spent)
    }

    /// Pre-flight check to see if a given amount can be afforded locally,
    /// without actually debiting the budget yet.
    pub fn verify_can_afford(&self, amount_sats: u64) -> Result<(), ClientError> {
        let current = self.spent_sats.load(Ordering::SeqCst);
        let next = current
            .checked_add(amount_sats)
            .ok_or_else(|| ClientError::BudgetExceeded {
                allocated: self.allocated_sats,
                needed: amount_sats,
            })?;

        if next > self.allocated_sats {
            return Err(ClientError::BudgetExceeded {
                allocated: self.allocated_sats,
                needed: amount_sats,
            });
        }

        Ok(())
    }
}
