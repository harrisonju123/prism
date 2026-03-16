use anyhow::Result;
use clap::{Parser, Subcommand};
use prism_cli::{persona, session::Session};

mod context;
use prism_client::PrismClient;

#[derive(Parser)]
#[command(
    name = "prism",
    about = "PrisM context management CLI"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List available personas
    Personas {
        #[command(subcommand)]
        cmd: PersonasCmd,
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
    /// Context management (threads, memories, decisions, agent coordination)
    Context {
        #[command(subcommand)]
        cmd: context::ContextCmd,
    },
}

#[derive(Subcommand)]
enum PersonasCmd {
    /// List all available personas
    List,
    /// Show details of a specific persona
    Show { name: String },
}

#[derive(Subcommand)]
enum SessionsCmd {
    /// List all saved sessions
    List,
    /// Delete a session by UUID prefix
    Rm { id_prefix: String },
    /// Show branch points in a session
    Branches { id_prefix: String },
}

fn main() {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let rt = tokio::runtime::Runtime::new().unwrap();
    if let Err(e) = rt.block_on(run(cli)) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Personas { cmd } => match cmd {
            PersonasCmd::List => {
                let personas = persona::list_personas(None);
                if personas.is_empty() {
                    eprintln!(
                        "no personas found. Create ~/.prism/personas/<name>.toml to get started."
                    );
                } else {
                    eprintln!("{:<20} {}", "NAME", "PATH");
                    for (name, path) in &personas {
                        eprintln!("{:<20} {}", name, path.display());
                    }
                }
            }
            PersonasCmd::Show { name } => {
                let p = persona::load_persona(&name, None)?;
                println!("{}", toml::to_string_pretty(&p)?);
            }
        },
        Commands::Models => {
            let config = prism_cli::config::Config::from_env()?;
            let client =
                PrismClient::new(&config.gateway.url).with_api_key(&config.gateway.api_key);
            let models = client.list_models().await?;
            println!("{}", serde_json::to_string_pretty(&models.data)?);
        }
        Commands::Health => {
            let config = prism_cli::config::Config::from_env()?;
            let client =
                PrismClient::new(&config.gateway.url).with_api_key(&config.gateway.api_key);
            let ok = client.health_check().await?;
            if ok {
                println!("gateway healthy");
            } else {
                println!("gateway unhealthy");
                std::process::exit(1);
            }
        }
        Commands::Context { cmd } => {
            context::run(cmd).await?;
        }
        Commands::Sessions { cmd } => {
            let sessions_dir = std::env::var("PRISM_SESSIONS_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| prism_cli::config::prism_home().join("sessions"));
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
                SessionsCmd::Branches { id_prefix } => {
                    let session = Session::load_by_id_prefix(&sessions_dir, &id_prefix)?;
                    let tree = session.tree.as_ref().ok_or_else(|| {
                        anyhow::anyhow!("session has no conversation tree (v1 format)")
                    })?;
                    let points = tree.branch_points();
                    if points.is_empty() {
                        eprintln!("no branch points in session");
                    } else {
                        eprintln!("{:<10} {}", "NODE", "BRANCHES");
                        for (parent_id, branches) in &points {
                            let branch_desc: Vec<String> = branches
                                .iter()
                                .map(|b| {
                                    format!("node {} ({}, depth {})", b.node_id, b.role, b.depth)
                                })
                                .collect();
                            eprintln!("{:<10} {}", parent_id, branch_desc.join(" | "));
                        }
                    }
                }
                SessionsCmd::Rm { id_prefix } => {
                    Session::delete(&sessions_dir, {
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
