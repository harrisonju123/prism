use anyhow::Result;
use clap::{Parser, Subcommand};
use prism_cli::{
    acp, agent::Agent, config::Config, mcp, memory, permissions::PermissionMode, persona, repl,
    session::Session, skills::SkillRegistry,
};
use prism_client::{PrismClient, RetryConfig};
use std::io::IsTerminal;
use std::sync::Arc;

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
        #[arg(long, help = "Persona name to load from ~/.prism/personas/<name>.toml")]
        persona: Option<String>,
        #[arg(
            long,
            help = "Resume a previous session. Omit value for most recent; pass UUID prefix for specific."
        )]
        resume: Option<Option<String>>,
        #[arg(long, value_enum)]
        permission_mode: Option<PermissionMode>,
        #[arg(
            long,
            help = "Plan file path: enables structural guardrail enforcement in plan mode (only this file may be written)"
        )]
        plan_file: Option<String>,
        #[arg(
            long,
            help = "Undo last assistant turn before resuming (requires --resume)"
        )]
        undo: bool,
        #[arg(long, help = "Switch to branch N before resuming (requires --resume)")]
        branch: Option<u32>,
    },
    /// List and manage agent personas
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

    // Logs go to stderr — stdout is the JSON-RPC protocol channel in ACP mode
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
                config.model.model = m;
            }
            let mcp_registry = load_mcp_registry(&config).await;
            let cwd = std::env::current_dir().unwrap_or_default();
            let skill_registry = SkillRegistry::discover(&cwd);
            acp::run_acp_server(config, mcp_registry, skill_registry).await?;
        }
        Commands::Run {
            task,
            model,
            max_turns,
            cost_cap,
            system,
            persona: persona_name,
            resume,
            permission_mode,
            plan_file,
            undo,
            branch,
        } => {
            let mut config = Config::from_env()?;

            // Apply persona first (lowest priority), then CLI overrides
            let effective_persona = persona_name.or_else(|| config.session.persona.clone());
            if let Some(ref name) = effective_persona {
                let p = persona::load_persona(name)
                    .map_err(|e| anyhow::anyhow!("failed to load persona '{}': {}", name, e))?;
                eprintln!(
                    "[persona] loaded '{}' — {}",
                    p.name,
                    p.description.as_deref().unwrap_or("")
                );
                p.apply(&mut config);
            }

            if let Some(m) = model {
                config.model.model = m;
            }
            if let Some(t) = max_turns {
                config.model.max_turns = t;
            }
            if let Some(c) = cost_cap {
                config.model.max_cost_usd = Some(c);
            }
            if let Some(s) = system {
                config.model.system_prompt = Some(s);
            }
            if let Some(pm) = permission_mode {
                config.extensions.permission_mode = Some(pm);
            }
            if let Some(pf) = plan_file {
                config.extensions.plan_file = Some(pf);
            }
            let client = PrismClient::new(&config.gateway.url)
                .with_api_key(&config.gateway.api_key)
                .with_retry_config(RetryConfig::with_max_retries(config.gateway.max_retries));
            let mcp_registry = load_mcp_registry(&config).await;
            let memory = load_memory().await;
            let cwd = std::env::current_dir().unwrap_or_default();
            let skill_registry = SkillRegistry::discover(&cwd);

            if (undo || branch.is_some()) && resume.is_none() {
                anyhow::bail!("--undo and --branch require --resume");
            }

            if let Some(resume_flag) = resume {
                // Resume a previous session
                let id_prefix = resume_flag.unwrap_or_else(|| "last".to_string());
                let mut session =
                    Session::load_by_id_prefix(&config.session.sessions_dir, &id_prefix)?;

                if let Some(node_id) = branch {
                    session.switch_branch(node_id);
                    eprintln!("[branch] switched to branch at node {node_id}");
                }

                if undo {
                    let removed = session.undo();
                    eprintln!("[undo] removed {removed} messages (last assistant turn)");
                }

                if std::io::stdin().is_terminal() && task.is_none() {
                    // TTY + resume + no explicit task → interactive mode
                    repl::run_interactive(
                        client,
                        config,
                        Some(session),
                        mcp_registry,
                        memory,
                        skill_registry,
                    )
                    .await?;
                } else {
                    eprintln!(
                        "[resume] episode {}  {} turns so far",
                        &session.episode_id.to_string()[..8],
                        session.turns
                    );
                    let mut agent = Agent::from_session(
                        client,
                        config,
                        session,
                        mcp_registry,
                        memory,
                        skill_registry,
                    );
                    let task_str = task.as_deref().unwrap_or("");
                    agent.resume(task_str).await?;
                }
            } else if let Some(task_str) = task {
                // Explicit task → single-shot (expand skill if needed)
                let task_str = if let Some((skill_name, skill_args)) =
                    prism_cli::skills::parse_skill_invocation(&task_str)
                {
                    match skill_registry.get(skill_name) {
                        Some(skill) => {
                            if !skill.user_invocable {
                                anyhow::bail!("skill '{skill_name}' is not user-invocable");
                            }
                            eprintln!("[skill] expanding /{skill_name}");
                            skill.expand(skill_args)
                        }
                        None => {
                            anyhow::bail!(
                                "unknown skill: '{skill_name}'. Available: {:?}",
                                skill_registry.names()
                            );
                        }
                    }
                } else {
                    task_str
                };

                let mut agent = Agent::new(
                    client,
                    config,
                    &task_str,
                    mcp_registry,
                    memory,
                    skill_registry,
                );
                agent.run(&task_str).await?;
            } else if std::io::stdin().is_terminal() {
                // No task, TTY → interactive mode
                repl::run_interactive(client, config, None, mcp_registry, memory, skill_registry)
                    .await?;
            } else {
                anyhow::bail!("task is required when stdin is not a terminal");
            }
        }
        Commands::Personas { cmd } => match cmd {
            PersonasCmd::List => {
                let personas = persona::list_personas();
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
                let p = persona::load_persona(&name)?;
                println!("{}", toml::to_string_pretty(&p)?);
            }
        },
        Commands::Models => {
            let config = Config::from_env()?;
            let client =
                PrismClient::new(&config.gateway.url).with_api_key(&config.gateway.api_key);
            let models = client.list_models().await?;
            println!("{}", serde_json::to_string_pretty(&models.data)?);
        }
        Commands::Health => {
            let config = Config::from_env()?;
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
                thread: None,
                constraints: None,
                handoff_mode: None,
            };
            let result = prism_cli::agent::spawn::spawn_agent(
                spawn_config,
                &config.gateway.url,
                &config.gateway.api_key,
            )
            .await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Commands::Sessions { cmd } => {
            // For sessions subcommand, use sessions_dir from env or default; don't require API key
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

async fn load_memory() -> memory::MemoryManager {
    let (store, workspace_id) = memory::open_context_store(None).await;
    memory::MemoryManager::new(store, workspace_id)
}

async fn load_mcp_registry(config: &Config) -> Option<Arc<mcp::McpRegistry>> {
    match mcp::config::load_mcp_config(&config.extensions.mcp_config_path) {
        Ok(mcp_config) if !mcp_config.mcp_servers.is_empty() => {
            let registry = mcp::McpRegistry::connect_all(&mcp_config).await;
            if registry.is_empty() {
                None
            } else {
                Some(Arc::new(registry))
            }
        }
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("failed to load MCP config: {e}");
            None
        }
    }
}
