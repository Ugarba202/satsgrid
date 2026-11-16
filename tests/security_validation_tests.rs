use nostr_sdk::prelude::*;
use satsgrid::discovery::announcement::NodeAnnouncement;
use satsgrid::discovery::security::EndpointValidator;

#[test]
fn test_ssrf_endpoint_validation_rules() {
    let strict_validator = EndpointValidator::new(false);

    // 1. Prohibited loopback and private IPs
    assert!(strict_validator
        .validate_url("http://127.0.0.1:8080")
        .is_err());
    assert!(strict_validator
        .validate_url("http://localhost:8080")
        .is_err());
    assert!(strict_validator
        .validate_url("http://10.0.0.1:8080")
        .is_err());
    assert!(strict_validator.validate_url("http://192.168.1.1").is_err());
    assert!(strict_validator.validate_url("http://172.16.0.1").is_err());

    // 2. Cloud metadata endpoint
    assert!(strict_validator
        .validate_url("http://169.254.169.254/latest/meta-data/")
        .is_err());

    // 3. Prohibited schemes
    assert!(strict_validator
        .validate_url("ftp://node.satsgrid.net")
        .is_err());
    assert!(strict_validator.validate_url("file:///etc/passwd").is_err());
    assert!(strict_validator
        .validate_url("gopher://node.satsgrid.net")
        .is_err());

    // 4. Embedded credentials prohibited
    assert!(strict_validator
        .validate_url("http://admin:secret@node.satsgrid.net")
        .is_err());

    // 5. Valid public HTTPS and HTTP URLs accepted
    assert!(strict_validator
        .validate_url("https://ai.satsgrid.net:8080")
        .is_ok());
    assert!(strict_validator
        .validate_url("http://node1.satsgrid.org")
        .is_ok());

    // 6. Permissive local validator allows loopback
    let local_validator = EndpointValidator::new(true);
    assert!(local_validator
        .validate_url("http://127.0.0.1:8080")
        .is_ok());
    assert!(local_validator
        .validate_url("http://localhost:8080")
        .is_ok());

    // 7. IPv6 unique local (ULA) and link-local rejection
    assert!(strict_validator
        .validate_url("http://[fc00::1]:8080")
        .is_err());
    assert!(strict_validator
        .validate_url("http://[fe80::1]:8080")
        .is_err());
    assert!(strict_validator.validate_url("http://[::1]:8080").is_err());
    assert!(strict_validator
        .validate_url("http://[::ffff:127.0.0.1]:8080")
        .is_err());
}

#[test]
fn test_tampered_event_signature_rejected() {
    let keys = Keys::generate();
    let announcement = NodeAnnouncement::new(
        "Tamper Node",
        "https://tamper.satsgrid.net:8080",
        vec!["llama3.2".to_string()],
        10,
        "testnet",
    );

    let builder = announcement.to_event_builder().expect("build event");
    let event = builder.finalize(&keys).expect("sign event");

    // Valid event verifies cleanly
    assert!(NodeAnnouncement::from_event(&event).is_ok());

    // Tampered event content should fail signature verification
    let mut tampered = event.clone();
    tampered.content = "{\"service\":\"satsgrid\",\"name\":\"Hacked Node\"}".to_string();
    assert!(NodeAnnouncement::from_event(&tampered).is_err());
}
