use satsgrid::client::budget::ClientBudgetTracker;
use satsgrid::client::error::ClientError;
use std::sync::Arc;
use tokio::task;

#[test]
fn test_budget_spending_normal() {
    let tracker = ClientBudgetTracker::new(500);

    // Spend 10
    let result = tracker.spend(10);
    assert!(result.is_ok());
    assert_eq!(tracker.remaining(), 490);
}

#[test]
fn test_budget_exhaustion_rejected() {
    let tracker = ClientBudgetTracker::new(10);

    // Spend 20 (more than available)
    let result = tracker.spend(20);
    assert!(result.is_err());

    match result.unwrap_err() {
        ClientError::BudgetExceeded { allocated, needed } => {
            assert_eq!(allocated, 10);
            assert_eq!(needed, 20);
        }
        other => panic!("Expected BudgetExceeded, got {:?}", other),
    }

    // Remaining should still be 10, not -10 or corrupted
    assert_eq!(tracker.remaining(), 10);
}

#[tokio::test]
async fn test_budget_concurrent_spending_safety() {
    // 100 budget
    let tracker = Arc::new(ClientBudgetTracker::new(100));

    // 10 concurrent tasks, each trying to spend 20.
    // If there were a race condition, many tasks might see 100 > 20 and succeed,
    // resulting in > 100 total spent.
    let mut handles = vec![];

    for _ in 0..10 {
        let tracker_clone = tracker.clone();
        handles.push(task::spawn(async move { tracker_clone.spend(20) }));
    }

    let mut success_count = 0;
    let mut failure_count = 0;

    for handle in handles {
        let result = handle.await.unwrap();
        if result.is_ok() {
            success_count += 1;
        } else {
            failure_count += 1;
        }
    }

    // Since budget is 100 and each spend is 20, exactly 5 should succeed!
    assert_eq!(success_count, 5);
    assert_eq!(failure_count, 5);

    // Remaining budget should be exactly 0
    assert_eq!(tracker.remaining(), 0);
}
