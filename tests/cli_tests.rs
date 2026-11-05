use std::path::Path;
use std::process::Command;

#[tokio::test]
async fn test_cli_help() {
    let output = Command::new(env!("CARGO_BIN_EXE_satsgrid"))
        .arg("--help")
        .output()
        .expect("Failed to execute infernos binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Permissionless Open-Model Inference Paid in Sats via L402"));
    assert!(stdout.contains("node"));
    assert!(stdout.contains("call"));
}

#[tokio::test]
async fn test_cli_node_status_stopped() {
    // Make sure no node is running and PID is clear
    let _ = std::fs::remove_file(Path::new(".satsgrid/node.pid"));

    let output = Command::new(env!("CARGO_BIN_EXE_satsgrid"))
        .arg("node")
        .arg("status")
        .output()
        .expect("Failed to execute infernos binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Status:       STOPPED"));
}

// Additional tests for node start/stop might be flaky if they don't clean up properly,
// so we'll just test the binary commands output instead of spinning up actual ports for now.
