use anyhow::Result;
use clap::{Parser, Subcommand};
use prism_cli::{acp, agent::Agent, config::Config, session::Session};
use prism_client::PrismClient;

#[derive(Parser)]
#[command(
    name = "prism",
    about = "PrisM Code Agent — Claude Code-style CLI powered by PrisM gateway"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run an agent session with a natural-language task
    Run {
        /// Task description (omit when resuming with no new instruction)
        task: Option<String>,
        #[arg(long, help = "Model to use (overrides PRISM_MODEL)")]
        model: Option<String>,
        #[arg(long, help = "Max agent turns (overrides PRISM_MAX_TURNS)")]
        max_turns: Option<u32>,
        #[arg(long, help = "Cost cap in USD (overrides PRISM_MAX_COST_USD)")]
        cost_cap: Option<f64>,
        #[arg(long, help = "Custom system prompt (overrides default)")]
        system: Option<String>,
        #[arg(
            long,
            help = "Resume a previous session. Omit value for most recent; pass UUID prefix for specific."
        )]
        resume: Option<Option<String>>,
    },
    /// List available models via the PrisM gateway
    Models,
    /// Check gateway health
    Health,
    /// Manage saved agent sessions
    Sessions {
        #[command(subcommand)]
        cmd: SessionsCmd,
    },
    /// Run as an ACP agent server (stdio protocol mode for Zed)
    Acp {
        #[arg(long, help = "Model to use (overrides PRISM_MODEL)")]
        model: Option<String>,
    },
    /// Spawn a sub-agent to execute a task and wait for result
    Spawn {
        /// Task description for the sub-agent
        task: String,
        #[arg(long, help = "Model to use")]
        model: Option<String>,
        #[arg(long, help = "Cost cap in USD")]
        cost_cap: Option<f64>,
        #[arg(long, help = "Timeout in seconds (default 300)")]
        timeout: Option<u64>,
    },
}

#[derive(Subcommand)]
enum SessionsCmd {
    /// List all saved sessions
    List,
    /// Delete a session by UUID prefix
    Rm { id_prefix: String },
}

fn main() {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    // ACP mode needs a current_thread runtime because Agent trait is !Send
    if matches!(&cli.command, Commands::Acp { .. }) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local_set = tokio::task::LocalSet::new();
        if let Err(e) = rt.block_on(local_set.run_until(run(cli))) {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
        return;
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    if let Err(e) = rt.block_on(run(cli)) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Acp { model } => {
            let mut config = Config::from_env()?;
            if let Some(m) = model {
                config.prism_model = m;
            }
            acp::run_acp_server(config).await?;
        }
        Commands::Run {
            task,
            model,
            max_turns,
            cost_cap,
            system,
            resume,
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
            let client = PrismClient::new(&config.prism_url).with_api_key(&config.prism_api_key);

            if let Some(resume_flag) = resume {
                // Resume a previous session
                let id_prefix = resume_flag.unwrap_or_else(|| "last".to_string());
                let session = Session::load_by_id_prefix(&config.sessions_dir, &id_prefix)?;
                eprintln!(
                    "[resume] episode {}  {} turns so far",
                    session.episode_id.to_string()[..8].to_string(),
                    session.turns
                );
                let mut agent = Agent::from_session(client, config, session);
                let task_str = task.as_deref().unwrap_or("");
                agent.resume(task_str).await?;
            } else {
                // New session
                let task_str =
                    task.ok_or_else(|| anyhow::anyhow!("task is required for new sessions"))?;
                let mut agent = Agent::new(client, config, &task_str);
                agent.run(&task_str).await?;
            }
        }
        Commands::Models => {
            let config = Config::from_env()?;
            let client = PrismClient::new(&config.prism_url).with_api_key(&config.prism_api_key);
            let models = client.list_models().await?;
            println!("{}", serde_json::to_string_pretty(&models.data)?);
        }
        Commands::Health => {
            let config = Config::from_env()?;
            let client = PrismClient::new(&config.prism_url).with_api_key(&config.prism_api_key);
            let ok = client.health_check().await?;
            if ok {
                println!("gateway healthy");
            } else {
                println!("gateway unhealthy");
                std::process::exit(1);
            }
        }
        Commands::Spawn {
            task,
            model,
            cost_cap,
            timeout,
        } => {
            let config = Config::from_env()?;
            let spawn_config = prism_cli::agent::spawn::SpawnConfig {
                task,
                model,
                cost_cap,
                tools: None,
                timeout_secs: timeout,
            };
            let result = prism_cli::agent::spawn::spawn_agent(
                spawn_config,
                &config.prism_url,
                &config.prism_api_key,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Sessions { cmd } => {
            // For sessions subcommand, use sessions_dir from env or default; don't require API key
            let sessions_dir = std::env::var("PRISM_SESSIONS_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    dirs::home_dir()
                        .unwrap_or_else(|| std::path::PathBuf::from("."))
                        .join(".prism/sessions")
                });
            match cmd {
                SessionsCmd::List => {
                    let summaries = Session::list_all(&sessions_dir)?;
                    if summaries.is_empty() {
                        eprintln!("no sessions");
                        return Ok(());
                    }
                    eprintln!(
                        "{:<10} {:<20} {:<50} {:<20} {:<6} {:<9} {}",
                        "ID", "UPDATED", "TASK", "MODEL", "TURNS", "COST", "STATUS"
                    );
                    for s in summaries {
                        let id_str = s.episode_id.to_string()[..8].to_string();
                        let status = s.stop_reason.as_deref().unwrap_or("active");
                        eprintln!(
                            "{:<10} {:<20} {:<50} {:<20} {:<6} ${:<8.4} {}",
                            id_str,
                            s.updated_at.get(..16).unwrap_or(&s.updated_at),
                            s.task,
                            s.model,
                            s.turns,
                            s.total_cost_usd,
                            status
                        );
                    }
                }
                SessionsCmd::Rm { id_prefix } => {
                    Session::delete(&sessions_dir, {
                        // Load to get the full ID for deletion
                        let summary = Session::load_by_id_prefix(&sessions_dir, &id_prefix)
                            .map(|s| s.episode_id)?;
                        summary
                    })?;
                    eprintln!("deleted session {id_prefix}");
                }
            }
        }
    }
    Ok(())
}
