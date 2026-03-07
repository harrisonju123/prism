use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use uglyhat::client::HttpClient;
use uglyhat::middleware::auth::hash_key;
use uglyhat::model::*;
use uglyhat::store::sqlite::SqliteStore;
use uglyhat::store::{ActivityFilters, Store, TaskFilters};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

const CONFIG_FILE: &str = ".uglyhat.json";
const DB_FILE: &str = ".uglyhat.db";

#[derive(Debug, Serialize, Deserialize)]
struct Config {
    workspace_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    api_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    base_url: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    mode: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    db_path: String,
}

fn find_config() -> Result<PathBuf, String> {
    let mut dir = std::env::current_dir().map_err(|e| e.to_string())?;
    loop {
        let path = dir.join(CONFIG_FILE);
        if path.exists() {
            return Ok(path);
        }
        if !dir.pop() {
            return Err(format!("no {CONFIG_FILE} found (run 'uh init' first)"));
        }
    }
}

fn load_config() -> Result<(Config, PathBuf), String> {
    let path = find_config()?;
    let data = std::fs::read_to_string(&path).map_err(|e| format!("reading config: {e}"))?;
    let cfg: Config = serde_json::from_str(&data).map_err(|e| format!("parsing config: {e}"))?;
    if cfg.workspace_id.is_empty() {
        return Err("config missing workspace_id".to_string());
    }
    let config_dir = path.parent().unwrap().to_path_buf();
    Ok((cfg, config_dir))
}

fn save_config(dir: &Path, cfg: &Config) -> Result<(), String> {
    let data = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(CONFIG_FILE), format!("{data}\n"))
        .map_err(|e| format!("writing config: {e}"))
}

fn resolve_db_path(cfg: &Config, config_dir: &Path) -> PathBuf {
    let db = if cfg.db_path.is_empty() {
        DB_FILE.to_string()
    } else {
        cfg.db_path.clone()
    };
    let p = Path::new(&db);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        config_dir.join(p)
    }
}

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "uh", about = "uglyhat CLI — AI-agent-first project management")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Bootstrap a new workspace
    Init {
        /// Workspace name
        name: String,
    },
    /// Workspace overview
    Context,
    /// Prioritized unassigned tasks
    Next {
        /// Max tasks to return
        #[arg(long, default_value = "5")]
        limit: i64,
    },
    /// Report an issue
    Report {
        /// Issue title
        title: String,
        /// Issue description
        #[arg(long)]
        desc: Option<String>,
        /// Severity (critical/high/medium/low)
        #[arg(long)]
        severity: Option<String>,
        /// Reporting source
        #[arg(long)]
        source: Option<String>,
        /// Comma-separated domain tags
        #[arg(long)]
        tags: Option<String>,
    },
    /// Initiative commands
    Initiative {
        #[command(subcommand)]
        action: InitiativeAction,
    },
    /// Epic commands
    Epic {
        #[command(subcommand)]
        action: EpicAction,
    },
    /// Task commands
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },
    /// List tasks with filters
    Tasks {
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
        /// Filter by domain tag
        #[arg(long)]
        domain: Option<String>,
        /// Filter by assignee
        #[arg(long)]
        assignee: Option<String>,
        /// Show only unassigned tasks
        #[arg(long)]
        unassigned: bool,
    },
    /// Decision commands
    Decision {
        #[command(subcommand)]
        action: DecisionAction,
    },
    /// Create a note
    Note {
        /// Note title
        title: String,
        /// Note content
        #[arg(long)]
        content: Option<String>,
        /// Attach to task ID
        #[arg(long)]
        task_id: Option<String>,
    },
    /// View activity log
    Activity {
        /// Filter by RFC3339 timestamp
        #[arg(long)]
        since: Option<String>,
        /// Filter by actor name
        #[arg(long)]
        actor: Option<String>,
        /// Max entries to return
        #[arg(long, default_value = "50")]
        limit: i64,
    },
    /// Agent check-in
    Checkin {
        /// Agent name (required)
        #[arg(long)]
        name: String,
        /// Comma-separated capabilities
        #[arg(long)]
        capabilities: Option<String>,
    },
    /// Agent check-out
    Checkout {
        /// Agent name (required)
        #[arg(long)]
        name: String,
        /// Session summary
        #[arg(long, default_value = "")]
        summary: String,
        /// Auto-complete the agent's current task on checkout
        #[arg(long)]
        complete_tasks: bool,
    },
    /// List agents and their current tasks
    Agents,
    /// Show stale tasks (in_progress with no active agent session)
    Stale,
    /// Create a structured handoff
    Handoff {
        /// Task ID
        task_id: String,
        /// Handoff summary
        #[arg(long, default_value = "")]
        summary: String,
        /// Comma-separated findings
        #[arg(long)]
        findings: Option<String>,
        /// Comma-separated blockers
        #[arg(long)]
        blockers: Option<String>,
        /// Comma-separated next steps
        #[arg(long)]
        next_steps: Option<String>,
    },
}

#[derive(Subcommand)]
enum InitiativeAction {
    /// Create an initiative
    Create {
        /// Initiative name
        name: String,
        /// Description
        #[arg(long)]
        desc: Option<String>,
    },
    /// List initiatives
    List,
}

#[derive(Subcommand)]
enum EpicAction {
    /// Create an epic
    Create {
        /// Epic name
        name: String,
        /// Initiative ID (required)
        #[arg(long)]
        initiative: String,
        /// Description
        #[arg(long)]
        desc: Option<String>,
    },
    /// List epics for an initiative
    List {
        /// Initiative ID (required)
        #[arg(long)]
        initiative: String,
    },
}

#[derive(Subcommand)]
enum TaskAction {
    /// Get a task by ID
    Get {
        /// Task ID
        id: String,
    },
    /// Update a task
    Update {
        /// Task ID
        id: String,
        /// Task status
        #[arg(long)]
        status: Option<String>,
        /// Assignee
        #[arg(long)]
        assignee: Option<String>,
        /// Priority
        #[arg(long)]
        priority: Option<String>,
        /// Task name
        #[arg(long)]
        name: Option<String>,
        /// Task description
        #[arg(long)]
        desc: Option<String>,
    },
    /// Create a task
    Create {
        /// Task name
        name: String,
        /// Epic ID (required)
        #[arg(long)]
        epic: String,
        /// Description
        #[arg(long)]
        desc: Option<String>,
        /// Priority
        #[arg(long, default_value = "medium")]
        priority: String,
        /// Assignee
        #[arg(long)]
        assignee: Option<String>,
        /// Comma-separated domain tags
        #[arg(long)]
        tags: Option<String>,
        /// Initial status
        #[arg(long, default_value = "backlog")]
        status: String,
    },
    /// Claim a task
    Claim {
        /// Task ID
        id: String,
        /// Agent name (required)
        #[arg(long)]
        name: String,
    },
    /// Show dependencies for a task
    Deps {
        /// Task ID
        id: String,
    },
    /// Add a blocking dependency
    Block {
        /// Blocking task ID
        blocking_id: String,
        /// Blocked task ID
        blocked_id: String,
    },
    /// Get rich context for a task
    Context {
        /// Task ID
        id: String,
    },
}

#[derive(Subcommand)]
enum DecisionAction {
    /// Create a decision
    Create {
        /// Decision title
        title: String,
        /// Decision content
        #[arg(long)]
        content: Option<String>,
        /// Initiative ID
        #[arg(long)]
        initiative: Option<String>,
        /// Epic ID
        #[arg(long)]
        epic: Option<String>,
    },
    /// List decisions
    List,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    let rt = tokio::runtime::Runtime::new().unwrap();
    if let Err(e) = rt.block_on(run(cli)) {
        let err_json = serde_json::json!({"error": e});
        eprintln!("{}", serde_json::to_string(&err_json).unwrap());
        process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Commands::Init { name } => cmd_init(&name).await,
        other => {
            let (cfg, config_dir) = load_config()?;
            let ws_id: Uuid = cfg
                .workspace_id
                .parse()
                .map_err(|e| format!("invalid workspace_id: {e}"))?;
            if !cfg.base_url.is_empty() {
                let client =
                    HttpClient::new(cfg.base_url.clone(), cfg.api_key.clone(), ws_id);
                run_command_remote(other, &client).await
            } else {
                let db_path = resolve_db_path(&cfg, &config_dir);
                let store = SqliteStore::open(db_path.to_str().unwrap())
                    .await
                    .map_err(|e| format!("opening database: {e}"))?;
                run_command_local(other, &store, ws_id).await
            }
        }
    }
}

async fn run_command_local(cmd: Commands, store: &SqliteStore, ws_id: Uuid) -> Result<(), String> {
    match cmd {
        Commands::Init { .. } => unreachable!(),
        Commands::Context => {
            let result = store
                .get_workspace_context(ws_id)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Next { limit } => {
            let result = store
                .get_next_tasks(ws_id, limit)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Report {
            title,
            desc,
            severity,
            source,
            tags,
        } => {
            let priority = match severity.as_deref() {
                Some("critical") => TaskPriority::Critical,
                Some("high") => TaskPriority::High,
                Some("low") => TaskPriority::Low,
                _ => TaskPriority::Medium,
            };

            let epic_id = store
                .get_system_epic_id(ws_id)
                .await
                .map_err(|e| e.to_string())?;

            let mut domain_tags = split_csv(tags);
            domain_tags.push("agent-reported".to_string());

            let mut meta = serde_json::Map::new();
            if let Some(ref src) = source {
                meta.insert(
                    "reported_by".to_string(),
                    serde_json::Value::String(src.clone()),
                );
            }
            meta.insert(
                "issue_type".to_string(),
                serde_json::Value::String("agent-reported".to_string()),
            );

            let task = store
                .create_task(
                    epic_id,
                    &title,
                    desc.as_deref().unwrap_or(""),
                    TaskStatus::Backlog,
                    priority,
                    source.as_deref().unwrap_or(""),
                    domain_tags,
                    Some(serde_json::Value::Object(meta)),
                )
                .await
                .map_err(|e| e.to_string())?;
            print_json(&task);
        }
        Commands::Initiative { action } => match action {
            InitiativeAction::Create { name, desc } => {
                let result = store
                    .create_initiative(ws_id, &name, desc.as_deref().unwrap_or(""), None)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            InitiativeAction::List => {
                let result = store
                    .list_initiatives_by_workspace(ws_id)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
        },
        Commands::Epic { action } => match action {
            EpicAction::Create {
                name,
                initiative,
                desc,
            } => {
                let init_id: Uuid = initiative
                    .parse()
                    .map_err(|e| format!("invalid initiative ID: {e}"))?;
                let result = store
                    .create_epic(init_id, &name, desc.as_deref().unwrap_or(""), None)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            EpicAction::List { initiative } => {
                let init_id: Uuid = initiative
                    .parse()
                    .map_err(|e| format!("invalid initiative ID: {e}"))?;
                let result = store
                    .list_epics_by_initiative(init_id)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
        },
        Commands::Task { action } => match action {
            TaskAction::Get { id } => {
                let task_id: Uuid = id.parse().map_err(|e| format!("invalid task ID: {e}"))?;
                let result = store.get_task(task_id).await.map_err(|e| e.to_string())?;
                print_json(&result);
            }
            TaskAction::Update {
                id,
                status,
                assignee,
                priority,
                name,
                desc,
            } => {
                let task_id: Uuid = id.parse().map_err(|e| format!("invalid task ID: {e}"))?;

                // GET current task
                let current = store.get_task(task_id).await.map_err(|e| e.to_string())?;

                let new_name = name.as_deref().unwrap_or(&current.name);
                let new_desc = desc.as_deref().unwrap_or(&current.description);
                let new_assignee = assignee.as_deref().unwrap_or(&current.assignee);

                let new_status = if let Some(ref s) = status {
                    parse_status(s)?
                } else {
                    current.status.clone()
                };
                let new_priority = if let Some(ref p) = priority {
                    parse_priority(p)?
                } else {
                    current.priority.clone()
                };

                let result = store
                    .update_task(
                        task_id,
                        new_name,
                        new_desc,
                        new_status,
                        new_priority,
                        new_assignee,
                        current.domain_tags.clone(),
                        current.metadata.clone(),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            TaskAction::Create {
                name,
                epic,
                desc,
                priority,
                assignee,
                tags,
                status,
            } => {
                let epic_id: Uuid = epic.parse().map_err(|e| format!("invalid epic ID: {e}"))?;
                let task_status = parse_status(&status)?;
                let task_priority = parse_priority(&priority)?;
                let domain_tags = split_csv(tags);

                let result = store
                    .create_task(
                        epic_id,
                        &name,
                        desc.as_deref().unwrap_or(""),
                        task_status,
                        task_priority,
                        assignee.as_deref().unwrap_or(""),
                        domain_tags,
                        None,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            TaskAction::Claim { id, name } => {
                let task_id: Uuid = id.parse().map_err(|e| format!("invalid task ID: {e}"))?;
                let result = store
                    .claim_task(ws_id, task_id, &name)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            TaskAction::Deps { id } => {
                let task_id: Uuid = id.parse().map_err(|e| format!("invalid task ID: {e}"))?;
                let (blocks, blocked_by) = store
                    .get_dependencies(task_id)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&serde_json::json!({
                    "task_id": task_id,
                    "blocks": blocks,
                    "blocked_by": blocked_by,
                }));
            }
            TaskAction::Block {
                blocking_id,
                blocked_id,
            } => {
                let blocking: Uuid = blocking_id
                    .parse()
                    .map_err(|e| format!("invalid blocking task ID: {e}"))?;
                let blocked: Uuid = blocked_id
                    .parse()
                    .map_err(|e| format!("invalid blocked task ID: {e}"))?;
                let result = store
                    .add_dependency(blocking, blocked)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            TaskAction::Context { id } => {
                let task_id: Uuid = id.parse().map_err(|e| format!("invalid task ID: {e}"))?;
                let result = store
                    .get_task_context(task_id)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
        },
        Commands::Tasks {
            status,
            domain,
            assignee,
            unassigned,
        } => {
            let filters = TaskFilters {
                status: status.as_deref().map(parse_status).transpose()?,
                priority: None,
                domain,
                assignee,
                unassigned: if unassigned { Some(true) } else { None },
            };
            let result = store
                .list_tasks_by_workspace(ws_id, filters)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Decision { action } => match action {
            DecisionAction::Create {
                title,
                content,
                initiative,
                epic,
            } => {
                let init_id: Option<Uuid> = initiative
                    .map(|s| s.parse().map_err(|e| format!("invalid initiative ID: {e}")))
                    .transpose()?;
                let epic_id: Option<Uuid> = epic
                    .map(|s| s.parse().map_err(|e| format!("invalid epic ID: {e}")))
                    .transpose()?;
                let result = store
                    .create_decision(
                        Some(ws_id),
                        init_id,
                        epic_id,
                        &title,
                        content.as_deref().unwrap_or(""),
                        None,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            DecisionAction::List => {
                let result = store
                    .list_decisions_by_workspace(ws_id)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
        },
        Commands::Note {
            title,
            content,
            task_id,
        } => {
            let tid: Option<Uuid> = task_id
                .map(|s| s.parse().map_err(|e| format!("invalid task ID: {e}")))
                .transpose()?;
            // If no parent specified, attach to workspace
            let (ws, init, ep, tk, dec) = if tid.is_some() {
                (None, None, None, tid, None)
            } else {
                (Some(ws_id), None, None, None, None)
            };
            let result = store
                .create_note(
                    ws,
                    init,
                    ep,
                    tk,
                    dec,
                    &title,
                    content.as_deref().unwrap_or(""),
                    None,
                )
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Activity {
            since,
            actor,
            limit,
        } => {
            let since_dt = since
                .map(|s| {
                    chrono::DateTime::parse_from_rfc3339(&s)
                        .map(|dt| dt.with_timezone(&chrono::Utc))
                        .map_err(|e| format!("invalid timestamp: {e}"))
                })
                .transpose()?;
            let filters = ActivityFilters {
                since: since_dt,
                actor,
                entity_type: None,
                limit,
            };
            let result = store
                .list_activity(ws_id, filters)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Agents => {
            let statuses = store.list_agent_statuses(ws_id).await.map_err(|e| e.to_string())?;
            print_json(&statuses);
        }
        Commands::Checkin { name, capabilities } => {
            let caps = split_csv(capabilities);
            let result = store
                .checkin_agent(ws_id, &name, caps)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Checkout { name, summary, complete_tasks } => {
            let result = store
                .checkout_agent(ws_id, &name, &summary, complete_tasks)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Stale => {
            let result = store
                .get_stale_tasks(ws_id)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Handoff {
            task_id,
            summary,
            findings,
            blockers,
            next_steps,
        } => {
            let tid: Uuid = task_id
                .parse()
                .map_err(|e| format!("invalid task ID: {e}"))?;
            let agent = std::env::var("UH_AGENT_NAME").unwrap_or_default();
            let f = split_csv(findings);
            let b = split_csv(blockers);
            let n = split_csv(next_steps);
            let result = store
                .create_handoff(tid, &agent, &summary, f, b, n, None)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Remote dispatch (HTTP client mode)
// ---------------------------------------------------------------------------

async fn run_command_remote(cmd: Commands, client: &HttpClient) -> Result<(), String> {
    match cmd {
        Commands::Init { .. } => unreachable!(),
        Commands::Context => {
            let result = client.get_context().await.map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Next { limit } => {
            let result = client.get_next_tasks(limit).await.map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Report {
            title,
            desc,
            severity,
            source,
            tags,
        } => {
            let domain_tags: Vec<String> = split_csv(tags);
            let body = serde_json::json!({
                "title": title,
                "description": desc.unwrap_or_default(),
                "severity": severity.unwrap_or_default(),
                "source": source.unwrap_or_default(),
                "domain_tags": domain_tags,
            });
            let result = client.report_issue(&body).await.map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Initiative { action } => match action {
            InitiativeAction::Create { name, desc } => {
                let result = client
                    .create_initiative(&name, desc.as_deref().unwrap_or(""))
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            InitiativeAction::List => {
                let result = client.list_initiatives().await.map_err(|e| e.to_string())?;
                print_json(&result);
            }
        },
        Commands::Epic { action } => match action {
            EpicAction::Create {
                name,
                initiative,
                desc,
            } => {
                let init_id: Uuid = initiative
                    .parse()
                    .map_err(|e| format!("invalid initiative ID: {e}"))?;
                let result = client
                    .create_epic(init_id, &name, desc.as_deref().unwrap_or(""))
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            EpicAction::List { initiative } => {
                let init_id: Uuid = initiative
                    .parse()
                    .map_err(|e| format!("invalid initiative ID: {e}"))?;
                let result = client.list_epics(init_id).await.map_err(|e| e.to_string())?;
                print_json(&result);
            }
        },
        Commands::Task { action } => match action {
            TaskAction::Get { id } => {
                let task_id: Uuid = id.parse().map_err(|e| format!("invalid task ID: {e}"))?;
                let result = client.get_task(task_id).await.map_err(|e| e.to_string())?;
                print_json(&result);
            }
            TaskAction::Update {
                id,
                status,
                assignee,
                priority,
                name,
                desc,
            } => {
                let task_id: Uuid = id.parse().map_err(|e| format!("invalid task ID: {e}"))?;
                let current = client.get_task(task_id).await.map_err(|e| e.to_string())?;
                let new_name = name.as_deref().unwrap_or(&current.name);
                let new_desc = desc.as_deref().unwrap_or(&current.description);
                let new_assignee = assignee.as_deref().unwrap_or(&current.assignee);
                let new_status = if let Some(ref s) = status {
                    parse_status(s)?
                } else {
                    current.status.clone()
                };
                let new_priority = if let Some(ref p) = priority {
                    parse_priority(p)?
                } else {
                    current.priority.clone()
                };
                let body = serde_json::json!({
                    "name": new_name,
                    "description": new_desc,
                    "status": new_status,
                    "priority": new_priority,
                    "assignee": new_assignee,
                    "domain_tags": current.domain_tags,
                    "metadata": current.metadata,
                });
                let result = client
                    .update_task(task_id, &body)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            TaskAction::Create {
                name,
                epic,
                desc,
                priority,
                assignee,
                tags,
                status,
            } => {
                let epic_id: Uuid = epic.parse().map_err(|e| format!("invalid epic ID: {e}"))?;
                let domain_tags = split_csv(tags);
                let result = client
                    .create_task(
                        epic_id,
                        &name,
                        desc.as_deref().unwrap_or(""),
                        &status,
                        &priority,
                        assignee.as_deref().unwrap_or(""),
                        domain_tags,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            TaskAction::Claim { id, name } => {
                let task_id: Uuid = id.parse().map_err(|e| format!("invalid task ID: {e}"))?;
                let result = client
                    .claim_task(task_id, &name)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            TaskAction::Deps { id } => {
                let task_id: Uuid = id.parse().map_err(|e| format!("invalid task ID: {e}"))?;
                let result = client.get_task_deps(task_id).await.map_err(|e| e.to_string())?;
                print_json(&result);
            }
            TaskAction::Block {
                blocking_id,
                blocked_id,
            } => {
                let blocking: Uuid = blocking_id
                    .parse()
                    .map_err(|e| format!("invalid blocking task ID: {e}"))?;
                let blocked: Uuid = blocked_id
                    .parse()
                    .map_err(|e| format!("invalid blocked task ID: {e}"))?;
                let result = client
                    .add_dependency(blocking, blocked)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
            TaskAction::Context { id } => {
                let task_id: Uuid = id.parse().map_err(|e| format!("invalid task ID: {e}"))?;
                let result = client
                    .get_task_context(task_id)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&result);
            }
        },
        Commands::Tasks {
            status,
            domain,
            assignee,
            unassigned,
        } => {
            let result = client
                .list_tasks(
                    status.as_deref(),
                    domain.as_deref(),
                    assignee.as_deref(),
                    unassigned,
                )
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Decision { action } => match action {
            DecisionAction::Create {
                title,
                content,
                initiative,
                epic,
            } => {
                let init_id: Option<Uuid> = initiative
                    .map(|s| s.parse().map_err(|e| format!("invalid initiative ID: {e}")))
                    .transpose()?;
                let epic_id: Option<Uuid> = epic
                    .map(|s| s.parse().map_err(|e| format!("invalid epic ID: {e}")))
                    .transpose()?;
                let body = serde_json::json!({
                    "title": title,
                    "content": content.unwrap_or_default(),
                    "initiative_id": init_id,
                    "epic_id": epic_id,
                });
                let result = client.create_decision(&body).await.map_err(|e| e.to_string())?;
                print_json(&result);
            }
            DecisionAction::List => {
                let result = client.list_decisions().await.map_err(|e| e.to_string())?;
                print_json(&result);
            }
        },
        Commands::Note {
            title,
            content,
            task_id,
        } => {
            let tid: Option<Uuid> = task_id
                .map(|s| s.parse().map_err(|e| format!("invalid task ID: {e}")))
                .transpose()?;
            let body = serde_json::json!({
                "title": title,
                "content": content.unwrap_or_default(),
                "task_id": tid,
            });
            let result = client.create_note(&body).await.map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Activity {
            since,
            actor,
            limit,
        } => {
            let result = client
                .list_activity(since.as_deref(), actor.as_deref(), limit)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Agents => {
            let result = client.list_agent_statuses().await.map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Checkin { name, capabilities } => {
            let caps = split_csv(capabilities);
            let result = client.checkin(&name, caps).await.map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Checkout { name, summary, complete_tasks } => {
            let result = client
                .checkout(&name, &summary, complete_tasks)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Stale => {
            let result = client.get_stale_tasks().await.map_err(|e| e.to_string())?;
            print_json(&result);
        }
        Commands::Handoff {
            task_id,
            summary,
            findings,
            blockers,
            next_steps,
        } => {
            let tid: Uuid = task_id
                .parse()
                .map_err(|e| format!("invalid task ID: {e}"))?;
            let agent = std::env::var("UH_AGENT_NAME").unwrap_or_default();
            let body = serde_json::json!({
                "task_id": tid,
                "agent_name": agent,
                "summary": summary,
                "findings": split_csv(findings),
                "blockers": split_csv(blockers),
                "next_steps": split_csv(next_steps),
            });
            let result = client.create_handoff(&body).await.map_err(|e| e.to_string())?;
            print_json(&result);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Init command
// ---------------------------------------------------------------------------

async fn cmd_init(name: &str) -> Result<(), String> {
    let dir = std::env::current_dir().map_err(|e| e.to_string())?;
    let db_path = dir.join(DB_FILE);

    let store = SqliteStore::open(db_path.to_str().unwrap())
        .await
        .map_err(|e| format!("creating database: {e}"))?;

    // Generate random API key
    let raw_key = generate_key();
    let key_hash = hash_key(&raw_key);
    let key_prefix = &raw_key[..8];

    let result = store
        .bootstrap_workspace(name, "", &key_hash, key_prefix)
        .await
        .map_err(|e| format!("bootstrapping workspace: {e}"))?;

    let cfg = Config {
        workspace_id: result.workspace.id.to_string(),
        api_key: raw_key.clone(),
        base_url: String::new(),
        mode: "local".to_string(),
        db_path: DB_FILE.to_string(),
    };
    save_config(&dir, &cfg)?;

    let resp = serde_json::json!({
        "workspace": result.workspace,
        "system_initiative_id": result.initiative_id,
        "system_epic_id": result.epic_id,
        "api_key": {
            "id": result.api_key.id,
            "workspace_id": result.api_key.workspace_id,
            "name": result.api_key.name,
            "key_prefix": result.api_key.key_prefix,
            "key": raw_key,
            "created_at": result.api_key.created_at,
        },
    });
    println!("{}", serde_json::to_string_pretty(&resp).unwrap());
    eprintln!("\nWrote {CONFIG_FILE} (local mode)");
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn print_json<T: Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
    );
}

fn split_csv(s: Option<String>) -> Vec<String> {
    s.map(|v| v.split(',').map(|x| x.trim().to_string()).collect())
        .unwrap_or_default()
}

fn parse_status(s: &str) -> Result<TaskStatus, String> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|_| format!("invalid status: {s}"))
}

fn parse_priority(s: &str) -> Result<TaskPriority, String> {
    serde_json::from_value(serde_json::Value::String(s.to_string()))
        .map_err(|_| format!("invalid priority: {s}"))
}

fn generate_key() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE.encode(bytes)
}

