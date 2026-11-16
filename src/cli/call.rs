use crate::client::budget::ClientBudgetTracker;
use crate::client::error::ClientError;
use crate::client::lib::SatsGridClient;
use crate::client::openai::{ChatCompletionRequest, ChatMessage};
use crate::client::pay::LightningPaymentProvider;
use clap::Args;
use std::sync::Arc;

#[derive(Args, Debug)]
pub struct CallArgs {
    #[arg(long, default_value = "http://127.0.0.1:8080")]
    pub node: String,

    #[arg(long)]
    pub model: String,

    #[arg(long)]
    pub prompt: String,

    #[arg(long, default_value_t = 100)]
    pub budget: u64,

    /// Optional LND REST host to pay invoices automatically (e.g. https://127.0.0.1:8081)
    #[arg(long)]
    pub payer_lnd_host: Option<String>,

    /// Optional path to payer LND admin.macaroon
    #[arg(long)]
    pub payer_macaroon: Option<String>,

    /// Optional path to payer LND tls.cert
    #[arg(long)]
    pub payer_tls_cert: Option<String>,

    /// Optional Nostr Wallet Connect (NWC) URI to pay invoices automatically (nostr+walletconnect://...)
    #[arg(long)]
    pub payer_nwc_uri: Option<String>,
}

struct InteractiveOrMockPaymentProvider {
    node_url: String,
    http_client: reqwest::Client,
}

impl InteractiveOrMockPaymentProvider {
    fn new(node_url: String) -> Self {
        Self {
            node_url,
            http_client: reqwest::Client::new(),
        }
    }
}

#[async_trait::async_trait]
impl LightningPaymentProvider for InteractiveOrMockPaymentProvider {
    async fn pay_invoice(&self, invoice: &str) -> Result<String, ClientError> {
        let url = format!("{}/internal/mock/pay", self.node_url.trim_end_matches('/'));

        let response = self
            .http_client
            .post(&url)
            .json(&serde_json::json!({
                "invoice": invoice
            }))
            .send()
            .await;

        if let Ok(resp) = response {
            if resp.status().is_success() {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    if let Some(preimage) = body["preimage"].as_str() {
                        return Ok(preimage.to_string());
                    }
                }
            }
        }

        // Mock payment not supported on this node (Node is running real Lightning / Testnet / Mainnet)
        println!("\n⚡ Lightning Payment Required (L402)");
        println!("────────────────────────────────────────────────────────────");
        println!("Invoice:\n{}", invoice);
        println!("────────────────────────────────────────────────────────────");
        println!("Please pay this invoice using your Lightning wallet (Testnet/Mainnet/LND).");
        print!("Enter payment preimage (hex): ");
        use std::io::{self, Write};
        let _ = io::stdout().flush();

        let mut input = String::new();
        io::stdin().read_line(&mut input).map_err(|e| {
            ClientError::Payment(format!("Failed to read preimage from stdin: {}", e))
        })?;

        let preimage = input.trim().to_string();
        if preimage.is_empty() {
            return Err(ClientError::Payment(
                "No preimage provided. Payment aborted.".to_string(),
            ));
        }

        Ok(preimage)
    }
}

pub async fn handle_call_command(args: CallArgs) {
    let payer_nwc_uri = args
        .payer_nwc_uri
        .clone()
        .or_else(|| std::env::var("SATSGRID_PAYER_NWC_URI").ok());
    let payer_host = args
        .payer_lnd_host
        .clone()
        .or_else(|| std::env::var("LND_PAYER_REST_HOST").ok());
    let payer_macaroon = args
        .payer_macaroon
        .clone()
        .or_else(|| std::env::var("LND_PAYER_MACAROON_PATH").ok());

    let (provider, payment_mode): (Arc<dyn LightningPaymentProvider>, &str) = if let Some(nwc_uri) =
        payer_nwc_uri
    {
        match crate::node::lightning::nwc::NwcLightningBackend::new(nwc_uri) {
            Ok(backend) => (Arc::new(backend), "NWC (Automated)"),
            Err(e) => {
                eprintln!(
                    "Warning: Failed to initialize payer NWC backend ({}), falling back to interactive payment.",
                    e
                );
                (
                    Arc::new(InteractiveOrMockPaymentProvider::new(args.node.clone())),
                    "Interactive Lightning",
                )
            }
        }
    } else if let (Some(host), Some(macaroon)) = (payer_host, payer_macaroon) {
        let cfg = crate::config::schema::LightningConfig {
            backend: crate::config::schema::LightningBackendType::Lnd,
            network: crate::config::schema::BitcoinNetwork::Testnet,
            lnd_rpc_host: Some(host),
            lnd_macaroon_path: Some(macaroon),
            lnd_tls_cert_path: args
                .payer_tls_cert
                .clone()
                .or_else(|| std::env::var("LND_PAYER_TLS_CERT_PATH").ok()),
            nwc_uri: None,
            ..Default::default()
        };
        match crate::node::lightning::lnd::LndBackend::from_config(&cfg) {
            Ok(backend) => (Arc::new(backend), "LND (Automated)"),
            Err(e) => {
                eprintln!("Warning: Failed to initialize payer LND backend ({}), falling back to interactive payment.", e);
                (
                    Arc::new(InteractiveOrMockPaymentProvider::new(args.node.clone())),
                    "Interactive Lightning",
                )
            }
        }
    } else {
        (
            Arc::new(InteractiveOrMockPaymentProvider::new(args.node.clone())),
            "Lightning (L402)",
        )
    };

    println!("SatsGrid\n──────────────────────────────");
    println!("Node:      {}", args.node);
    println!("Model:     {}", args.model);
    println!("Budget:    {} sats", args.budget);
    println!("Payment:   {}\n", payment_mode);

    let budget_tracker = Arc::new(ClientBudgetTracker::new(args.budget));

    let client = match SatsGridClient::builder()
        .node_url(args.node.clone())
        .payment_provider(provider)
        .build()
    {
        Ok(c) => c.with_budget(budget_tracker.clone()),
        Err(e) => {
            eprintln!("Failed to initialize client: {}", e);
            return;
        }
    };

    // Session creation
    if let Err(e) = client.create_session(args.budget).await {
        eprintln!("Failed to create session: {}", e);
        return;
    }
    println!("✓ Session created");
    println!("✓ L402 authorization established");
    println!("✓ Inference request accepted\n");

    let req = ChatCompletionRequest {
        model: args.model,
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: args.prompt,
        }],
        stream: None,
    };

    println!("Response\n──────────────────────────────\n");

    match client.chat(&req).await {
        Ok(response) => {
            for choice in response.choices {
                println!("{}", choice.message.content);
            }

            println!("\n──────────────────────────────");
            let spent = args.budget.saturating_sub(budget_tracker.remaining());
            println!("Charged:    {} sats", spent);
            println!("Remaining:  {} sats", budget_tracker.remaining());
        }
        Err(e) => {
            eprintln!("Inference request failed: {}", e);
        }
    }
}
