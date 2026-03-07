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
        Commands::Run { task } => {
            let config = Config::from_env()?;
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
