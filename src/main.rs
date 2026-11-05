use clap::Parser;
use satsgrid::cli::{Cli, Commands};
use satsgrid::common::error::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Node(args) => {
            satsgrid::cli::node::handle_node_command(args).await;
        }
        Commands::Call(args) => {
            satsgrid::cli::call::handle_call_command(args).await;
        }
        Commands::Discover(args) => {
            if let Err(e) = satsgrid::cli::discover::handle_discover_command(args).await {
                eprintln!("Discovery error: {}", e);
            }
        }
    }

    Ok(())
}
