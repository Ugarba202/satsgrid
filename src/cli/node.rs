use crate::cli::process::ProcessManager;
use crate::config::schema::NodeConfig;
use crate::discovery::{NodeAnnouncement, NostrDiscoveryClient};
use crate::node::proxy::OpenAiProxy;
use crate::node::server::SatsGridServer;
use clap::{Args, Subcommand};
use nostr_sdk::prelude::Keys;
use reqwest::Client;
use std::process;
use std::time::Duration;

#[derive(Args, Debug)]
pub struct NodeArgs {
    #[command(subcommand)]
    pub command: NodeCommands,
}

#[derive(Subcommand, Debug)]
pub enum NodeCommands {
    /// Start the SatsGrid node
    Start {
        #[arg(short, long, default_value = "config/node.toml")]
        config: String,
        /// Broadcast node availability to Nostr relays on startup
        #[arg(long)]
        announce: bool,
    },
    /// Stop the SatsGrid node
    Stop,
    /// Show the status of the SatsGrid node
    Status,
    /// Announce the node to Nostr discovery relays
    Announce {
        #[arg(short, long, default_value = "config/node.toml")]
        config: String,
        /// Nostr relay websocket URLs (overrides config if provided)
        #[arg(long)]
        relay: Vec<String>,
        /// Optional nsec private key (if not provided, generates or uses config)
        #[arg(long)]
        nsec: Option<String>,
    },
}

pub async fn handle_node_command(args: NodeArgs) {
    match args.command {
        NodeCommands::Start { config, announce } => start(config, announce).await,
        NodeCommands::Stop => stop().await,
        NodeCommands::Status => status().await,
        NodeCommands::Announce {
            config,
            relay,
            nsec,
        } => announce(config, relay, nsec).await,
    }
}

async fn start(config_path: String, announce: bool) {
    println!("Starting SatsGrid node...");

    // Check if already running
    if let Some(pid) = ProcessManager::read_pid() {
        if ProcessManager::is_process_alive(pid) {
            eprintln!("Error: Node is already running with PID {}", pid);
            return;
        } else {
            // Clean up stale PID
            ProcessManager::remove_pid();
        }
    }

    let config = match NodeConfig::from_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load configuration: {}", e);
            return;
        }
    };
    println!("✓ Configuration loaded from {}", config_path);
    println!(
        "✓ Lightning backend: {:?} ({:?})",
        config.lightning.backend, config.lightning.network
    );
    println!("✓ Upstream: {}", config.upstream.url);
    println!(
        "✓ Server listening on {}:{}",
        config.server.host, config.server.port
    );

    // Announce to Nostr relays if requested or configured
    if announce || config.discovery.enabled {
        let conf_clone = config.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            announce_node(&conf_clone, &[], None).await;
        });
    }

    let server = SatsGridServer::new(config);

    // Write PID (the actual binary PID)
    let pid = process::id();
    if let Err(e) = ProcessManager::write_pid(pid) {
        eprintln!("Error writing PID file: {}", e);
        return;
    }

    println!("\nSatsGrid node is running.");

    // Create signal listener for graceful shutdown
    let shutdown_signal = async {
        let ctrl_c = async {
            let _ = tokio::signal::ctrl_c().await;
        };

        #[cfg(unix)]
        let terminate = async {
            if let Ok(mut sigterm) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            {
                sigterm.recv().await;
            } else {
                std::future::pending::<()>().await;
            }
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {}
            _ = terminate => {}
        }
        println!("\nShutting down...");
    };

    if let Err(e) = server.run_until_shutdown(shutdown_signal).await {
        eprintln!("Server error: {}", e);
    }

    ProcessManager::remove_pid();
}

async fn announce(config_path: String, custom_relays: Vec<String>, nsec_override: Option<String>) {
    let config = match NodeConfig::from_file(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load configuration: {}", e);
            return;
        }
    };

    announce_node(&config, &custom_relays, nsec_override.as_deref()).await;
}

async fn announce_node(config: &NodeConfig, custom_relays: &[String], nsec_override: Option<&str>) {
    let relays = if !custom_relays.is_empty() {
        custom_relays.to_vec()
    } else {
        config.discovery.relays.clone()
    };

    let keys = if let Some(key_str) = nsec_override {
        match Keys::parse(key_str) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("Invalid Nostr private key (nsec): {}", e);
                return;
            }
        }
    } else if let Some(key_str) = &config.discovery.nsec {
        match Keys::parse(key_str) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("Invalid Nostr private key in config: {}", e);
                return;
            }
        }
    } else {
        Keys::generate()
    };

    let node_name = config
        .discovery
        .node_name
        .clone()
        .unwrap_or_else(|| format!("satsgrid-node-{}", config.server.port));

    let node_url = config
        .discovery
        .public_url
        .clone()
        .unwrap_or_else(|| format!("http://{}:{}", config.server.host, config.server.port));

    // Try discovering models from upstream
    let proxy = OpenAiProxy::new(config.upstream.url.clone());
    let models = if let Ok(val) = proxy.list_models().await {
        if let Some(arr) = val.get("data").and_then(|d| d.as_array()) {
            arr.iter()
                .filter_map(|m| {
                    m.get("id")
                        .and_then(|id| id.as_str())
                        .map(|s| s.to_string())
                })
                .collect()
        } else {
            vec!["default".to_string()]
        }
    } else {
        vec!["default".to_string()]
    };

    let network = format!("{:?}", config.lightning.network).to_lowercase();
    let announcement = NodeAnnouncement::new(
        node_name,
        node_url.clone(),
        models.clone(),
        config.pricing.default_price_sats.0,
        network,
    );

    println!(
        "Broadcasting SatsGrid node announcement to Nostr relays: {:?}",
        relays
    );
    match NostrDiscoveryClient::publish_announcement(&relays, &keys, &announcement).await {
        Ok(event_id) => {
            println!("✓ Node announcement successfully published!");
            println!("  Event ID:    {}", event_id);
            println!("  Public Key:  {}", keys.public_key().to_hex());
            println!("  Endpoint:    {}", node_url);
            println!("  Models:      {:?}", models);
            println!(
                "  Price:       {} sats",
                config.pricing.default_price_sats.0
            );
        }
        Err(e) => {
            eprintln!("⚠ Failed to publish announcement: {}", e);
        }
    }
}

async fn status() {
    println!("SatsGrid Node\n─────────────");

    let pid_opt = ProcessManager::read_pid();
    if pid_opt.is_none() {
        println!("Status:       STOPPED");
        return;
    }

    let pid = pid_opt.unwrap();
    if !ProcessManager::is_process_alive(pid) {
        ProcessManager::remove_pid();
        println!("Status:       STOPPED");
        return;
    }

    let config =
        NodeConfig::from_file("config/node.toml").unwrap_or_else(|_| NodeConfig::default());

    // Check health endpoint
    let client = Client::new();
    let url = format!(
        "http://{}:{}/health",
        config.server.host, config.server.port
    );
    let health_check =
        matches!(client.get(&url).send().await, Ok(res) if res.status().is_success());

    if health_check {
        println!("Status:       RUNNING\n");
    } else {
        println!("Status:       UNHEALTHY\n");
    }

    println!("PID:          {}", pid);
    println!(
        "Endpoint:     {}:{}",
        config.server.host, config.server.port
    );
    println!("Upstream:     {}", config.upstream.url);
    println!(
        "Price:        {} sats/request",
        config.pricing.default_price_sats.0
    );
    println!(
        "Lightning:    {:?} ({:?})",
        config.lightning.backend, config.lightning.network
    );
}

async fn stop() {
    println!("Stopping SatsGrid node...");

    let pid_opt = ProcessManager::read_pid();
    if pid_opt.is_none() {
        println!("SatsGrid node is not running.");
        return;
    }

    let pid = pid_opt.unwrap();
    if !ProcessManager::is_process_alive(pid) {
        ProcessManager::remove_pid();
        println!("SatsGrid node is not running.");
        return;
    }

    match ProcessManager::stop_process(pid) {
        Ok(true) => {
            println!("✓ Shutdown signal sent");

            // Wait briefly
            for _ in 0..10 {
                if !ProcessManager::is_process_alive(pid) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            if ProcessManager::is_process_alive(pid) {
                println!("Node is taking too long to stop. It may need manual intervention.");
            } else {
                ProcessManager::remove_pid();
                println!("✓ Node stopped");
                println!("✓ PID file removed");
            }
        }
        _ => {
            eprintln!("Failed to send shutdown signal.");
        }
    }
}
