use std::process;

use chrono::Utc;
use clap::{Parser, Subcommand};
use serde::Serialize;
use uuid::Uuid;

use uglyhat::config::{self, Config, CONFIG_FILE};
use uglyhat::store::sqlite::SqliteStore;
use uglyhat::store::{ActivityFilters, MemoryFilters, Store};
use uglyhat::util::parse_duration;

fn agent_name() -> String {
    std::env::var("UH_AGENT_NAME").unwrap_or_else(|_| "claude".to_string())
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "uh", about = "Context management for AI agents")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Bootstrap a new workspace
    Init {
        name: String,
        #[arg(long, default_value = "")]
        desc: String,
    },
    /// Show workspace overview
    Context,

    /// Thread management
    Thread {
        #[command(subcommand)]
        action: ThreadAction,
    },

    /// Save a memory (upsert by key)
    Remember {
        key: String,
        value: String,
        #[arg(long)]
        thread: Option<String>,
        #[arg(long)]
        source: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },
    /// Delete a memory
    Forget { key: String },
    /// List memories
    Memories {
        #[arg(long)]
        thread: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
        #[arg(long)]
        global: bool,
    },

    /// Record a decision
    Decide {
        title: String,
        #[arg(long, default_value = "")]
        content: String,
        #[arg(long)]
        thread: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },
    /// List decisions
    Decisions {
        #[arg(long)]
        thread: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },

    /// Recall context (thread, tags, or time-based)
    Recall {
        /// Thread name to recall
        thread: Option<String>,
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
        /// Duration like "2h", "30m", "1d"
        #[arg(long)]
        since: Option<String>,
    },

    /// Agent checkin
    Checkin {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_delimiter = ',')]
        capabilities: Option<Vec<String>>,
        #[arg(long)]
        thread: Option<String>,
    },
    /// Agent checkout
    Checkout {
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "")]
        summary: String,
        #[arg(long, value_delimiter = ',')]
        findings: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        files: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        next_steps: Option<Vec<String>>,
    },
    /// List agents
    Agents,

    /// Activity log
    Activity {
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        actor: Option<String>,
        #[arg(long, default_value = "50")]
        limit: i64,
    },

    /// Create a point-in-time snapshot
    Snapshot {
        #[arg(long, default_value = "")]
        label: String,
    },
}

#[derive(Subcommand)]
enum ThreadAction {
    Create {
        name: String,
        #[arg(long, default_value = "")]
        desc: String,
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },
    List {
        #[arg(long)]
        active: bool,
        #[arg(long)]
        archived: bool,
    },
    Archive {
        name: String,
    },
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn print_json(val: &impl Serialize) {
    println!(
        "{}",
        serde_json::to_string_pretty(val).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
    );
}

async fn resolve_thread_id(
    store: &SqliteStore,
    workspace_id: Uuid,
    thread_name: &str,
) -> Option<Uuid> {
    store
        .get_thread(workspace_id, thread_name)
        .await
        .ok()
        .map(|t| t.id)
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli).await {
        eprintln!("error: {e}");
        process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    // Init is special — no config file yet
    if let Commands::Init { ref name, ref desc } = cli.command {
        return run_init(name, desc).await;
    }

    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let config_path = config::find_config(&cwd)
        .ok_or_else(|| format!("no {CONFIG_FILE} found (run 'uh init' first)"))?;
    let config = config::load_config(&config_path)?;
    let db = config::resolve_db_path(&config_path, &config)
        .to_string_lossy()
        .to_string();
    let workspace_id: Uuid = config
        .workspace_id
        .parse()
        .map_err(|e| format!("invalid workspace_id in config: {e}"))?;

    let store = SqliteStore::open(&db)
        .await
        .map_err(|e| format!("open db: {e}"))?;

    match cli.command {
        Commands::Init { .. } => unreachable!(),
        Commands::Context => {
            let overview = store
                .get_workspace_overview(workspace_id)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&overview);
        }
        Commands::Thread { action } => match action {
            ThreadAction::Create { name, desc, tags } => {
                let t = store
                    .create_thread(workspace_id, &name, &desc, tags.unwrap_or_default())
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&t);
            }
            ThreadAction::List { active, archived } => {
                let status = if active {
                    Some(uglyhat::model::ThreadStatus::Active)
                } else if archived {
                    Some(uglyhat::model::ThreadStatus::Archived)
                } else {
                    None
                };
                let threads = store
                    .list_threads(workspace_id, status)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&threads);
            }
            ThreadAction::Archive { name } => {
                let t = store
                    .archive_thread(workspace_id, &name)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&t);
            }
        },
        Commands::Remember {
            key,
            value,
            thread,
            source,
            tags,
        } => {
            let thread_id = if let Some(ref tname) = thread {
                resolve_thread_id(&store, workspace_id, tname).await
            } else {
                None
            };
            let src = source.unwrap_or_else(agent_name);
            let m = store
                .save_memory(
                    workspace_id,
                    &key,
                    &value,
                    thread_id,
                    &src,
                    tags.unwrap_or_default(),
                )
                .await
                .map_err(|e| e.to_string())?;
            print_json(&m);
        }
        Commands::Forget { key } => {
            store
                .delete_memory(workspace_id, &key)
                .await
                .map_err(|e| e.to_string())?;
            println!("{{\"deleted\":\"{key}\"}}");
        }
        Commands::Memories {
            thread,
            tags,
            global,
        } => {
            let thread_name = thread;
            let filters = MemoryFilters {
                thread_name,
                tags,
                global_only: global,
                ..Default::default()
            };
            let memories = store
                .load_memories(workspace_id, filters)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&memories);
        }
        Commands::Decide {
            title,
            content,
            thread,
            tags,
        } => {
            let thread_id = if let Some(ref tname) = thread {
                resolve_thread_id(&store, workspace_id, tname).await
            } else {
                None
            };
            let d = store
                .save_decision(
                    workspace_id,
                    &title,
                    &content,
                    thread_id,
                    tags.unwrap_or_default(),
                )
                .await
                .map_err(|e| e.to_string())?;
            print_json(&d);
        }
        Commands::Decisions { thread, tags } => {
            let thread_id = if let Some(ref tname) = thread {
                resolve_thread_id(&store, workspace_id, tname).await
            } else {
                None
            };
            let decisions = store
                .list_decisions(workspace_id, thread_id, tags)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&decisions);
        }
        Commands::Recall {
            thread,
            tags,
            since,
        } => {
            if let Some(ref tname) = thread {
                let ctx = store
                    .recall_thread(workspace_id, tname)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&ctx);
            } else if tags.is_some() || since.is_some() {
                let since_dt = if let Some(ref dur_str) = since {
                    let dur = parse_duration(dur_str)?;
                    Some(Utc::now() - dur)
                } else {
                    None
                };
                let result = store
                    .recall_by_tags(workspace_id, tags.unwrap_or_default(), since_dt)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            } else {
                return Err("recall requires --thread, --tags, or --since".to_string());
            }
        }
        Commands::Checkin {
            name,
            capabilities,
            thread,
        } => {
            let agent = name.unwrap_or_else(agent_name);
            let thread_id = if let Some(ref tname) = thread {
                resolve_thread_id(&store, workspace_id, tname).await
            } else {
                None
            };
            let ctx = store
                .checkin(
                    workspace_id,
                    &agent,
                    capabilities.unwrap_or_default(),
                    thread_id,
                )
                .await
                .map_err(|e| e.to_string())?;
            print_json(&ctx);
        }
        Commands::Checkout {
            name,
            summary,
            findings,
            files,
            next_steps,
        } => {
            let agent = name.unwrap_or_else(agent_name);
            let session = store
                .checkout(
                    workspace_id,
                    &agent,
                    &summary,
                    findings.unwrap_or_default(),
                    files.unwrap_or_default(),
                    next_steps.unwrap_or_default(),
                )
                .await
                .map_err(|e| e.to_string())?;
            print_json(&session);
        }
        Commands::Agents => {
            let agents = store
                .list_agents(workspace_id)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&agents);
        }
        Commands::Activity {
            since,
            actor,
            limit,
        } => {
            let since_dt = if let Some(ref dur_str) = since {
                let dur = parse_duration(dur_str)?;
                Some(Utc::now() - dur)
            } else {
                None
            };
            let filters = ActivityFilters {
                since: since_dt,
                actor,
                limit,
            };
            let activity = store
                .list_activity(workspace_id, filters)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&activity);
        }
        Commands::Snapshot { label } => {
            let snap = store
                .create_snapshot(workspace_id, &label)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&snap);
        }
    }

    Ok(())
}

async fn run_init(name: &str, desc: &str) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let config_path = cwd.join(CONFIG_FILE);
    let db_path = cwd.join(config::DB_FILE);

    if config_path.exists() {
        return Err(format!("{CONFIG_FILE} already exists"));
    }

    let store = SqliteStore::open(&db_path.to_string_lossy())
        .await
        .map_err(|e| format!("open db: {e}"))?;

    let workspace = store
        .init_workspace(name, desc)
        .await
        .map_err(|e| e.to_string())?;

    let config = Config {
        workspace_id: workspace.id.to_string(),
        db_path: String::new(),
    };

    let config_json =
        serde_json::to_string_pretty(&config).map_err(|e| format!("serialize config: {e}"))?;
    std::fs::write(&config_path, config_json).map_err(|e| format!("write config: {e}"))?;

    print_json(&workspace);
    eprintln!("workspace initialized: {}", workspace.name);
    Ok(())
}
