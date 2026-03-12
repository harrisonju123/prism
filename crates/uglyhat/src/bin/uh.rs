use std::process;

use chrono::Utc;
use clap::{Parser, Subcommand};
use serde::Serialize;
use uuid::Uuid;

use uglyhat::config::{self, CONFIG_FILE, Config};
use uglyhat::model::*;
use uglyhat::store::sqlite::SqliteStore;
use uglyhat::store::{ActivityFilters, InboxFilters, MemoryFilters, Store};
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
        /// Decision scope: 'thread' or 'workspace' (workspace notifies all agents)
        #[arg(long, default_value = "thread")]
        scope: String,
        /// Supersede an existing decision by ID
        #[arg(long)]
        supersede: Option<String>,
        /// Revoke a decision by ID (ignores title)
        #[arg(long)]
        revoke: Option<String>,
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

    /// Send a heartbeat
    Heartbeat {
        #[arg(long)]
        name: Option<String>,
    },

    /// Agent state and management
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },

    /// Handoff management
    Handoff {
        #[command(subcommand)]
        action: HandoffAction,
    },

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

    /// Supervisory feed (approvals, blockers, suggestions)
    Inbox {
        #[command(subcommand)]
        action: InboxAction,
    },

    /// Show agent-to-agent messages
    Messages {
        /// Show only unread messages
        #[arg(long)]
        unread: bool,
    },

    /// Send a message to an agent
    Send {
        /// Recipient agent name
        to: String,
        /// Message content
        message: String,
        /// Optional conversation UUID to group related messages
        #[arg(long)]
        conversation: Option<String>,
    },

    /// Plan management (intent-driven work decomposition)
    Plan {
        #[command(subcommand)]
        action: PlanAction,
    },

    /// Work package management
    Wp {
        #[command(subcommand)]
        action: WpAction,
    },

    /// File claim management (advisory locking)
    Files {
        #[command(subcommand)]
        action: FilesAction,
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
    /// Manage thread guardrails
    Guard {
        name: String,
        /// Show current guardrails
        #[arg(long)]
        show: bool,
        /// Remove lock
        #[arg(long)]
        unlock: bool,
        /// Set owner agent
        #[arg(long)]
        owner: Option<String>,
        /// Lock the thread
        #[arg(long)]
        lock: bool,
        /// Allowed file patterns (comma-separated)
        #[arg(long, value_delimiter = ',')]
        allowed_files: Option<Vec<String>>,
        /// Allowed tool names (comma-separated)
        #[arg(long, value_delimiter = ',')]
        allowed_tools: Option<Vec<String>>,
        /// Cost budget in USD
        #[arg(long)]
        cost_budget: Option<f64>,
    },
}

#[derive(Subcommand)]
enum AgentAction {
    /// Set agent state
    State {
        /// State: idle, working, blocked
        state: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Reap dead agents with stale heartbeats
    Reap {
        /// Timeout duration (e.g. '10m', '1h')
        #[arg(long, default_value = "10m")]
        timeout: String,
    },
}

#[derive(Subcommand)]
enum HandoffAction {
    /// Create a handoff task
    Create {
        task: String,
        #[arg(long)]
        thread: Option<String>,
        #[arg(long)]
        cost_cap: Option<f64>,
        #[arg(long)]
        timeout: Option<u64>,
        #[arg(long, default_value = "delegate-and-await")]
        mode: String,
        #[arg(long, value_delimiter = ',')]
        allowed_tools: Option<Vec<String>>,
        #[arg(long, value_delimiter = ',')]
        allowed_files: Option<Vec<String>>,
    },
    /// Accept a pending handoff
    Accept {
        id: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Complete a handoff with a result
    Complete {
        id: String,
        #[arg(long)]
        result: String,
    },
    /// List handoffs
    List {
        #[arg(long)]
        agent: Option<String>,
        #[arg(long)]
        status: Option<String>,
    },
}

#[derive(Subcommand)]
enum InboxAction {
    /// Create a supervisory inbox entry
    Create {
        /// Entry type: approval | blocked | suggestion | risk | cost_spike | completed
        #[arg(long)]
        r#type: String,
        /// Short title for the entry
        #[arg(long)]
        title: String,
        /// Optional longer description
        #[arg(long, default_value = "")]
        body: String,
        /// Severity: critical | warning | info
        #[arg(long, default_value = "info")]
        severity: String,
        /// Reference in the form "type:id" (e.g. "thread:my-thread" or "handoff:uuid")
        #[arg(long)]
        r#ref: Option<String>,
    },
    /// List inbox entries
    List {
        /// Show only unread entries
        #[arg(long)]
        unread: bool,
        /// Filter by entry type
        #[arg(long)]
        r#type: Option<String>,
        /// Include dismissed entries
        #[arg(long)]
        dismissed: bool,
    },
    /// Mark an entry as read
    Read { id: String },
    /// Dismiss an entry
    Dismiss { id: String },
}

#[derive(Subcommand)]
enum PlanAction {
    /// Create a new plan from an intent statement
    Create { intent: String },
    /// List plans
    List {
        /// Filter by status: draft | approved | active | completed | cancelled
        #[arg(long)]
        status: Option<String>,
    },
    /// Show a plan and its work packages
    Show { plan_id: String },
    /// Approve a plan (draft → approved)
    Approve { plan_id: String },
}

#[derive(Subcommand)]
enum WpAction {
    /// Add a work package to a plan
    Add {
        intent: String,
        #[arg(long)]
        plan: Option<String>,
        #[arg(long, value_delimiter = ',')]
        criteria: Option<Vec<String>>,
        /// UUID of work package this one depends on
        #[arg(long)]
        after: Option<String>,
        #[arg(long, default_value = "0")]
        ordinal: i32,
        #[arg(long, value_delimiter = ',')]
        tags: Option<Vec<String>>,
    },
    /// List work packages
    List {
        #[arg(long)]
        plan: Option<String>,
        /// Filter by status: draft | planned | ready | in_progress | review | done | cancelled
        #[arg(long)]
        status: Option<String>,
    },
    /// Update work package status
    Status {
        wp_id: String,
        /// New status: planned | ready | in_progress | review | done | cancelled
        status: String,
    },
}

#[derive(Subcommand)]
enum FilesAction {
    /// Claim a file for editing (advisory lock)
    Claim {
        path: String,
        /// TTL in seconds; claim auto-expires after this duration
        #[arg(long)]
        ttl: Option<i64>,
    },
    /// Release your claim on a file
    Release { path: String },
    /// Check if a file is claimed (exit 0=free, exit 1=claimed)
    Check { path: String },
    /// List active file claims
    List {
        /// Filter to a specific agent
        #[arg(long)]
        agent: Option<String>,
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
                    Some(ThreadStatus::Active)
                } else if archived {
                    Some(ThreadStatus::Archived)
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
            ThreadAction::Guard {
                name,
                show,
                unlock,
                owner,
                lock,
                allowed_files,
                allowed_tools,
                cost_budget,
            } => {
                if show {
                    let g = store
                        .get_guardrails(workspace_id, &name)
                        .await
                        .map_err(|e| e.to_string())?;
                    match g {
                        Some(guardrails) => print_json(&guardrails),
                        None => println!("{{\"guardrails\": null}}"),
                    }
                } else if unlock {
                    // Get existing guardrails, set locked=false
                    let existing = store
                        .get_guardrails(workspace_id, &name)
                        .await
                        .map_err(|e| e.to_string())?;
                    if let Some(mut g) = existing {
                        g.locked = false;
                        let updated = store
                            .set_guardrails(workspace_id, &name, g)
                            .await
                            .map_err(|e| e.to_string())?;
                        print_json(&updated);
                    } else {
                        println!("{{\"guardrails\": null}}");
                    }
                } else {
                    // Look up owner agent ID if provided
                    let owner_agent_id = if let Some(ref owner_name) = owner {
                        // Resolve agent name to ID via checkin (upsert)
                        let ctx = store
                            .checkin(workspace_id, owner_name, vec![], None)
                            .await
                            .map_err(|e| e.to_string())?;
                        Some(ctx.agent.id)
                    } else {
                        None
                    };

                    let thread = store
                        .get_thread(workspace_id, &name)
                        .await
                        .map_err(|e| e.to_string())?;

                    let g = ThreadGuardrails {
                        id: Uuid::new_v4(),
                        thread_id: thread.id,
                        workspace_id,
                        owner_agent_id,
                        locked: lock,
                        allowed_files: allowed_files.unwrap_or_default(),
                        allowed_tools: allowed_tools.unwrap_or_default(),
                        cost_budget_usd: cost_budget,
                        cost_spent_usd: 0.0,
                        created_at: Utc::now(),
                        updated_at: Utc::now(),
                    };
                    let result = store
                        .set_guardrails(workspace_id, &name, g)
                        .await
                        .map_err(|e| e.to_string())?;
                    print_json(&result);
                }
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
            scope,
            supersede,
            revoke,
        } => {
            if let Some(revoke_id) = revoke {
                let id: Uuid = revoke_id
                    .parse()
                    .map_err(|e| format!("invalid decision id: {e}"))?;
                let d = store
                    .revoke_decision(workspace_id, id)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&d);
            } else if let Some(old_id_str) = supersede {
                let old_id: Uuid = old_id_str
                    .parse()
                    .map_err(|e| format!("invalid decision id: {e}"))?;
                let thread_id = if let Some(ref tname) = thread {
                    resolve_thread_id(&store, workspace_id, tname).await
                } else {
                    None
                };
                let d = store
                    .supersede_decision(
                        workspace_id,
                        old_id,
                        &title,
                        &content,
                        thread_id,
                        tags.unwrap_or_default(),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&d);
            } else {
                let thread_id = if let Some(ref tname) = thread {
                    resolve_thread_id(&store, workspace_id, tname).await
                } else {
                    None
                };
                let decision_scope = DecisionScope::from_str(&scope).ok_or_else(|| {
                    format!("invalid scope: {scope} (use 'thread' or 'workspace')")
                })?;
                let d = store
                    .save_decision(
                        workspace_id,
                        &title,
                        &content,
                        thread_id,
                        tags.unwrap_or_default(),
                        decision_scope,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&d);
            }
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
        Commands::Heartbeat { name } => {
            let agent = name.unwrap_or_else(agent_name);
            store
                .heartbeat(workspace_id, &agent)
                .await
                .map_err(|e| e.to_string())?;
            println!("{{\"heartbeat\":\"{agent}\"}}");
        }
        Commands::Agent { action } => match action {
            AgentAction::State { state, name } => {
                let agent = name.unwrap_or_else(agent_name);
                let agent_state = AgentState::from_str(&state).ok_or_else(|| {
                    format!("invalid state: {state} (use idle, working, or blocked)")
                })?;
                store
                    .set_agent_state(workspace_id, &agent, agent_state)
                    .await
                    .map_err(|e| e.to_string())?;
                println!("{{\"agent\":\"{agent}\",\"state\":\"{state}\"}}");
            }
            AgentAction::Reap { timeout } => {
                let dur = parse_duration(&timeout)?;
                let timeout_secs = dur.num_seconds();
                let reaped = store
                    .reap_dead_agents(workspace_id, timeout_secs)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&serde_json::json!({"reaped": reaped}));
            }
        },
        Commands::Handoff { action } => match action {
            HandoffAction::Create {
                task,
                thread,
                cost_cap,
                timeout,
                mode,
                allowed_tools,
                allowed_files,
            } => {
                let from_agent = agent_name();
                let thread_id = if let Some(ref tname) = thread {
                    resolve_thread_id(&store, workspace_id, tname).await
                } else {
                    None
                };
                let handoff_mode = match mode.as_str() {
                    "delegate-and-forget" | "delegate_and_forget" => HandoffMode::DelegateAndForget,
                    _ => HandoffMode::DelegateAndAwait,
                };
                let constraints = HandoffConstraints {
                    cost_cap,
                    timeout_secs: timeout,
                    allowed_tools: allowed_tools.unwrap_or_default(),
                    allowed_files: allowed_files.unwrap_or_default(),
                };
                let h = store
                    .create_handoff(
                        workspace_id,
                        &from_agent,
                        &task,
                        thread_id,
                        constraints,
                        handoff_mode,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&h);
            }
            HandoffAction::Accept { id, name } => {
                let agent = name.unwrap_or_else(agent_name);
                let handoff_id: Uuid =
                    id.parse().map_err(|e| format!("invalid handoff id: {e}"))?;
                let h = store
                    .accept_handoff(workspace_id, handoff_id, &agent)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&h);
            }
            HandoffAction::Complete { id, result } => {
                let handoff_id: Uuid =
                    id.parse().map_err(|e| format!("invalid handoff id: {e}"))?;
                let result_json: serde_json::Value = serde_json::from_str(&result)
                    .map_err(|e| format!("invalid result JSON: {e}"))?;
                let h = store
                    .complete_handoff(workspace_id, handoff_id, result_json)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&h);
            }
            HandoffAction::List { agent, status } => {
                let handoff_status = status
                    .as_deref()
                    .map(|s| {
                        HandoffStatus::from_str(s).ok_or_else(|| format!("invalid status: {s}"))
                    })
                    .transpose()?;
                let handoffs = store
                    .list_handoffs(workspace_id, agent.as_deref(), handoff_status)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&handoffs);
            }
        },
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
        Commands::Inbox { action } => match action {
            InboxAction::Create {
                r#type,
                title,
                body,
                severity,
                r#ref,
            } => {
                let entry_type = InboxEntryType::from_str(&r#type)
                    .ok_or_else(|| format!("invalid type: {type} (use approval|blocked|suggestion|risk|cost_spike|completed)"))?;
                let sev = InboxSeverity::from_str(&severity).ok_or_else(|| {
                    format!("invalid severity: {severity} (use critical|warning|info)")
                })?;
                // Parse optional ref in the form "type:id"
                let (ref_type, ref_id) = if let Some(ref s) = r#ref {
                    match s.split_once(':') {
                        Some((rtype, rid)) => (Some(rtype.to_string()), rid.parse::<Uuid>().ok()),
                        None => {
                            return Err(format!(
                                "invalid --ref format: expected type:uuid, got {s:?}"
                            ));
                        }
                    }
                } else {
                    (None, None)
                };
                let entry = store
                    .create_inbox_entry(
                        workspace_id,
                        entry_type,
                        &title,
                        &body,
                        sev,
                        Some(&agent_name()),
                        ref_type.as_deref(),
                        ref_id,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&entry);
            }
            InboxAction::List {
                unread,
                r#type,
                dismissed,
            } => {
                let entry_type = r#type
                    .as_deref()
                    .map(|s| {
                        InboxEntryType::from_str(s).ok_or_else(|| format!("invalid type: {s}"))
                    })
                    .transpose()?;
                let filters = InboxFilters {
                    unread_only: unread,
                    entry_type,
                    include_dismissed: dismissed,
                    ..Default::default()
                };
                let entries = store
                    .list_inbox_entries(workspace_id, filters)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&entries);
            }
            InboxAction::Read { id } => {
                let entry_id: Uuid = id.parse().map_err(|e| format!("invalid id: {e}"))?;
                store
                    .mark_inbox_read(workspace_id, entry_id)
                    .await
                    .map_err(|e| e.to_string())?;
                println!("{{\"read\":\"{id}\"}}");
            }
            InboxAction::Dismiss { id } => {
                let entry_id: Uuid = id.parse().map_err(|e| format!("invalid id: {e}"))?;
                store
                    .dismiss_inbox_entry(workspace_id, entry_id)
                    .await
                    .map_err(|e| e.to_string())?;
                println!("{{\"dismissed\":\"{id}\"}}");
            }
        },
        Commands::Messages { unread } => {
            let messages = store
                .list_messages(workspace_id, &agent_name(), unread)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&messages);
        }
        Commands::Send {
            to,
            message,
            conversation,
        } => {
            let conv_id = conversation
                .as_deref()
                .map(|s| s.parse::<Uuid>().map_err(|e| format!("invalid conversation id: {e}")))
                .transpose()?;
            let msg = store
                .send_message(workspace_id, &agent_name(), &to, &message, conv_id)
                .await
                .map_err(|e| e.to_string())?;
            print_json(&msg);
        }
        Commands::Plan { action } => match action {
            PlanAction::Create { intent } => {
                let plan = store
                    .create_plan(workspace_id, &intent)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&plan);
            }
            PlanAction::List { status } => {
                let plan_status = status
                    .as_deref()
                    .map(|s| PlanStatus::from_str(s).ok_or_else(|| format!("invalid status: {s}")))
                    .transpose()?;
                let plans = store
                    .list_plans(workspace_id, plan_status)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&plans);
            }
            PlanAction::Show { plan_id } => {
                let pid: Uuid = plan_id
                    .parse()
                    .map_err(|e| format!("invalid plan id: {e}"))?;
                let plan = store
                    .get_plan(workspace_id, pid)
                    .await
                    .map_err(|e| e.to_string())?;
                let wps = store
                    .list_work_packages(workspace_id, Some(pid), None)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&serde_json::json!({"plan": plan, "work_packages": wps}));
            }
            PlanAction::Approve { plan_id } => {
                let pid: Uuid = plan_id
                    .parse()
                    .map_err(|e| format!("invalid plan id: {e}"))?;
                let plan = store
                    .update_plan_status(workspace_id, pid, PlanStatus::Approved)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&plan);
            }
        },
        Commands::Files { action } => match action {
            FilesAction::Claim { path, ttl } => {
                let agent = agent_name();
                match store.claim_file(workspace_id, &agent, &path, ttl).await {
                    Ok(claim) => print_json(&claim),
                    Err(uglyhat::error::Error::Conflict(msg)) => {
                        eprintln!("WARNING: file claimed by another agent: {msg}");
                        process::exit(1);
                    }
                    Err(e) => return Err(e.to_string()),
                }
            }
            FilesAction::Release { path } => {
                let agent = agent_name();
                store
                    .release_file(workspace_id, &path, &agent)
                    .await
                    .map_err(|e| e.to_string())?;
                println!("{{\"released\":{:?}}}", path);
            }
            FilesAction::Check { path } => {
                let claim = store
                    .check_file_claim(workspace_id, &path)
                    .await
                    .map_err(|e| e.to_string())?;
                match claim {
                    Some(c) => {
                        print_json(&c);
                        process::exit(1);
                    }
                    None => {
                        println!("null");
                    }
                }
            }
            FilesAction::List { agent } => {
                let claims = store
                    .list_file_claims(workspace_id, agent.as_deref())
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&claims);
            }
        },
        Commands::Wp { action } => match action {
            WpAction::Add {
                intent,
                plan,
                criteria,
                after,
                ordinal,
                tags,
            } => {
                let plan_id = plan
                    .as_deref()
                    .map(|s| {
                        s.parse::<Uuid>()
                            .map_err(|e| format!("invalid plan id: {e}"))
                    })
                    .transpose()?;
                let depends_on = after
                    .as_deref()
                    .map(|s| s.parse::<Uuid>().map_err(|e| format!("invalid wp id: {e}")))
                    .transpose()?
                    .into_iter()
                    .collect::<Vec<_>>();
                let wp = store
                    .create_work_package(
                        workspace_id,
                        plan_id,
                        &intent,
                        criteria.unwrap_or_default(),
                        ordinal,
                        depends_on,
                        tags.unwrap_or_default(),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&wp);
            }
            WpAction::List { plan, status } => {
                let plan_id = plan
                    .as_deref()
                    .map(|s| {
                        s.parse::<Uuid>()
                            .map_err(|e| format!("invalid plan id: {e}"))
                    })
                    .transpose()?;
                let wp_status = status
                    .as_deref()
                    .map(|s| {
                        WorkPackageStatus::from_str(s).ok_or_else(|| format!("invalid status: {s}"))
                    })
                    .transpose()?;
                let wps = store
                    .list_work_packages(workspace_id, plan_id, wp_status)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&wps);
            }
            WpAction::Status { wp_id, status } => {
                let wid: Uuid = wp_id.parse().map_err(|e| format!("invalid wp id: {e}"))?;
                let wp_status = WorkPackageStatus::from_str(&status)
                    .ok_or_else(|| format!("invalid status: {status}"))?;
                let wp = store
                    .update_work_package_status(workspace_id, wid, wp_status)
                    .await
                    .map_err(|e| e.to_string())?;
                print_json(&wp);
            }
        },
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
