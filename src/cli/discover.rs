use crate::common::error::Result;
use crate::discovery::announcement::NodeAnnouncement;
use crate::discovery::client::NostrDiscoveryClient;
use clap::Args;
use std::time::Duration;

#[derive(Args, Debug, Clone)]
pub struct DiscoverArgs {
    /// Nostr relay websocket URLs to query (can be specified multiple times)
    #[arg(
        long,
        default_values_t = vec!["wss://relay.damus.io".to_string(), "wss://nos.lol".to_string()]
    )]
    pub relay: Vec<String>,

    /// Filter by specific AI model (e.g. llama3.2, mistral)
    #[arg(long)]
    pub model: Option<String>,

    /// Timeout in seconds to query relays
    #[arg(long, default_value_t = 5)]
    pub timeout_secs: u64,

    /// Output results as JSON
    #[arg(long)]
    pub json: bool,
}

pub async fn handle_discover_command(args: DiscoverArgs) -> Result<()> {
    if !args.json {
        let filter_info = match &args.model {
            Some(m) => format!(" serving '{}'", m),
            None => String::new(),
        };
        println!(
            "Searching Nostr relays for SatsGrid nodes{} (relays: {:?})...",
            filter_info, args.relay
        );
    }

    let nodes = NostrDiscoveryClient::discover_nodes(
        &args.relay,
        args.model.as_deref(),
        Duration::from_secs(args.timeout_secs),
    )
    .await?;

    if args.json {
        let json_str = serde_json::to_string_pretty(&nodes).map_err(|e| {
            crate::common::error::Error::Config(format!("Failed to format JSON: {}", e))
        })?;
        println!("{}", json_str);
        return Ok(());
    }

    if nodes.is_empty() {
        println!("\nNo active SatsGrid nodes discovered matching criteria.");
        println!("Tips:");
        println!("  - Ensure the relays are accessible");
        println!("  - Try omitting --model to see all nodes");
        println!("  - Check --timeout-secs if relays are slow to respond");
        return Ok(());
    }

    println!("\nDiscovered {} active SatsGrid node(s):\n", nodes.len());
    print_nodes_table(&nodes);

    Ok(())
}

fn print_nodes_table(nodes: &[NodeAnnouncement]) {
    println!(
        "{:<20} {:<30} {:<24} {:<15} {:<10}",
        "NODE NAME", "ENDPOINT", "MODELS", "PRICE (SATS)", "NETWORK"
    );
    println!("{}", "-".repeat(105));

    for node in nodes {
        let models_str = if node.models.is_empty() {
            "none".to_string()
        } else {
            node.models.join(", ")
        };
        let truncated_models = if models_str.len() > 22 {
            format!("{}...", &models_str[..19])
        } else {
            models_str
        };

        let truncated_name = if node.name.len() > 18 {
            format!("{}...", &node.name[..15])
        } else {
            node.name.clone()
        };

        let truncated_url = if node.node_url.len() > 28 {
            format!("{}...", &node.node_url[..25])
        } else {
            node.node_url.clone()
        };

        println!(
            "{:<20} {:<30} {:<24} {:<15} {:<10}",
            truncated_name, truncated_url, truncated_models, node.pricing_sats, node.network
        );
    }
    println!("{}", "-".repeat(105));
}
