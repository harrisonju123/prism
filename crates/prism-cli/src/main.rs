use anyhow::Result;
use clap::{Parser, Subcommand};
use prism_client::PrismClient;
use prism_cli::{agent::Agent, config::Config};

#[derive(Parser)]
#[command(name = "prism", about = "PrisM Code Agent — Claude Code-style CLI powered by PrisM gateway")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run an agent session with a natural-language task
    Run {
        /// Task description
        task: String,
        #[arg(long, help = "Model to use (overrides PRISM_MODEL)")]
        model: Option<String>,
        #[arg(long, help = "Max agent turns (overrides PRISM_MAX_TURNS)")]
        max_turns: Option<u32>,
        #[arg(long, help = "Cost cap in USD (overrides PRISM_MAX_COST_USD)")]
        cost_cap: Option<f64>,
        #[arg(long, help = "Custom system prompt (overrides default)")]
        system: Option<String>,
    },
    /// List available models via the PrisM gateway
    Models,
    /// Check gateway health
    Health,
}

fn main() {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let rt = tokio::runtime::Runtime::new().unwrap();
    if let Err(e) = rt.block_on(run(cli)) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Run {
            task,
            model,
            max_turns,
            cost_cap,
            system,
        } => {
            let mut config = Config::from_env()?;
            if let Some(m) = model {
                config.prism_model = m;
            }
            if let Some(t) = max_turns {
                config.max_turns = t;
            }
            if let Some(c) = cost_cap {
                config.max_cost_usd = Some(c);
            }
            if let Some(s) = system {
                config.system_prompt = Some(s);
            }
            let client = PrismClient::new(&config.prism_url)
                .with_api_key(&config.prism_api_key);
            let mut agent = Agent::new(client, config);
            agent.run(&task).await?;
        }
        Commands::Models => {
            let config = Config::from_env()?;
            let client = PrismClient::new(&config.prism_url)
                .with_api_key(&config.prism_api_key);
            let models = client.list_models().await?;
            println!("{}", serde_json::to_string_pretty(&models.data)?);
        }
        Commands::Health => {
            let config = Config::from_env()?;
            let client = PrismClient::new(&config.prism_url)
                .with_api_key(&config.prism_api_key);
            let ok = client.health_check().await?;
            if ok {
                println!("gateway healthy");
            } else {
                println!("gateway unhealthy");
                std::process::exit(1);
            }
        }
    }
    Ok(())
}
