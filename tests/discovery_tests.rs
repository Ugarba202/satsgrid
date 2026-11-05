use clap::Parser;
use nostr_sdk::prelude::*;
use satsgrid::cli::{Cli, Commands};
use satsgrid::discovery::announcement::{NodeAnnouncement, SATSGRID_DISCOVERY_KIND, SATSGRID_TAG};
use satsgrid::discovery::client::NostrDiscoveryClient;

#[test]
fn test_node_announcement_schema_and_tags() {
    let keys = Keys::generate();
    let announcement = NodeAnnouncement::new(
        "Alpha Node",
        "https://alpha.satsgrid.local:8080",
        vec![
            "llama3.2".to_string(),
            "mistral-7b".to_string(),
            "deepseek-r1".to_string(),
        ],
        25,
        "testnet",
    );

    let builder = announcement
        .to_event_builder()
        .expect("builder should construct");
    let event = builder.finalize(&keys).expect("event should sign");

    assert_eq!(event.kind, Kind::Custom(SATSGRID_DISCOVERY_KIND));
    assert_eq!(event.pubkey, keys.public_key());

    // Verify Nostr tags
    let tags_str: Vec<Vec<String>> = event.tags.iter().map(|t| t.as_slice().to_vec()).collect();

    assert!(tags_str
        .iter()
        .any(|t| t.first().map(|s| s.as_str()) == Some("t")
            && t.get(1).map(|s| s.as_str()) == Some(SATSGRID_TAG)));
    assert!(tags_str
        .iter()
        .any(|t| t.first().map(|s| s.as_str()) == Some("endpoint")
            && t.get(1).map(|s| s.as_str()) == Some("https://alpha.satsgrid.local:8080")));
    assert!(tags_str
        .iter()
        .any(|t| t.first().map(|s| s.as_str()) == Some("pricing")
            && t.get(1).map(|s| s.as_str()) == Some("25")));
    assert!(tags_str
        .iter()
        .any(|t| t.first().map(|s| s.as_str()) == Some("network")
            && t.get(1).map(|s| s.as_str()) == Some("testnet")));
    assert!(tags_str
        .iter()
        .any(|t| t.first().map(|s| s.as_str()) == Some("m")
            && t.get(1).map(|s| s.as_str()) == Some("llama3.2")));
    assert!(tags_str
        .iter()
        .any(|t| t.first().map(|s| s.as_str()) == Some("m")
            && t.get(1).map(|s| s.as_str()) == Some("mistral-7b")));
    assert!(tags_str
        .iter()
        .any(|t| t.first().map(|s| s.as_str()) == Some("m")
            && t.get(1).map(|s| s.as_str()) == Some("deepseek-r1")));

    // Parse back
    let parsed = NodeAnnouncement::from_event(&event).expect("should parse back from event");
    assert_eq!(parsed.name, "Alpha Node");
    assert_eq!(parsed.node_url, "https://alpha.satsgrid.local:8080");
    assert_eq!(parsed.models, vec!["llama3.2", "mistral-7b", "deepseek-r1"]);
    assert_eq!(parsed.pricing_sats, 25);
    assert_eq!(parsed.network, "testnet");
    assert_eq!(parsed.pubkey, Some(keys.public_key().to_hex()));
}

#[test]
fn test_node_announcement_rejects_wrong_kind() {
    let keys = Keys::generate();
    let builder = EventBuilder::new(Kind::TextNote, "hello nostr");
    let event = builder.finalize(&keys).expect("should sign");

    let err = NodeAnnouncement::from_event(&event).expect_err("should reject text note");
    assert!(err.to_string().contains("Unexpected event kind"));
}

#[test]
fn test_node_announcement_filter_generation() {
    let general_filter = NodeAnnouncement::create_filter(None);
    assert!(general_filter.kinds.is_some());

    let model_filter = NodeAnnouncement::create_filter(Some("llama3.2"));
    assert!(model_filter.kinds.is_some());
}

#[tokio::test]
async fn test_discovery_client_lifecycle() {
    let client = NostrDiscoveryClient::new();
    assert!(!client.client().is_shutdown());

    // Adding invalid relay URL should fail gracefully without panic
    let err = client.add_relay("not-a-valid-url").await;
    assert!(err.is_err());

    client.shutdown().await;
    assert!(client.client().is_shutdown());
}

#[test]
fn test_cli_discover_command_parsing() {
    let cli = Cli::try_parse_from([
        "satsgrid",
        "discover",
        "--relay",
        "wss://relay.damus.io",
        "--relay",
        "wss://nos.lol",
        "--model",
        "llama3.2",
        "--timeout-secs",
        "10",
        "--json",
    ])
    .expect("CLI should parse discover command");

    match cli.command {
        Commands::Discover(args) => {
            assert_eq!(
                args.relay,
                vec![
                    "wss://relay.damus.io".to_string(),
                    "wss://nos.lol".to_string()
                ]
            );
            assert_eq!(args.model.as_deref(), Some("llama3.2"));
            assert_eq!(args.timeout_secs, 10);
            assert!(args.json);
        }
        _ => panic!("Expected Commands::Discover"),
    }
}

#[test]
fn test_cli_node_announce_command_parsing() {
    let cli = Cli::try_parse_from([
        "satsgrid",
        "node",
        "announce",
        "--config",
        "config/node.toml",
        "--relay",
        "wss://relay.damus.io",
    ])
    .expect("CLI should parse node announce command");

    match cli.command {
        Commands::Node(args) => match args.command {
            satsgrid::cli::node::NodeCommands::Announce {
                config,
                relay,
                nsec,
            } => {
                assert_eq!(config, "config/node.toml");
                assert_eq!(relay, vec!["wss://relay.damus.io".to_string()]);
                assert_eq!(nsec, None);
            }
            _ => panic!("Expected NodeCommands::Announce"),
        },
        _ => panic!("Expected Commands::Node"),
    }
}

#[test]
fn test_cli_node_start_with_announce_parsing() {
    let cli = Cli::try_parse_from([
        "satsgrid",
        "node",
        "start",
        "--config",
        "config/node.toml",
        "--announce",
    ])
    .expect("CLI should parse node start --announce");

    match cli.command {
        Commands::Node(args) => match args.command {
            satsgrid::cli::node::NodeCommands::Start { config, announce } => {
                assert_eq!(config, "config/node.toml");
                assert!(announce);
            }
            _ => panic!("Expected NodeCommands::Start"),
        },
        _ => panic!("Expected Commands::Node"),
    }
}
