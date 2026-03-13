use anyhow::Result;
use clap::ValueEnum;
use prism_client::PrismClient;
use rustyline::EditMode;
use rustyline::completion::Completer;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::agent::Agent;
use crate::config::{Config, repl_history_path};
use crate::render::Renderer;
use crate::mcp::McpRegistry;
use crate::memory::MemoryManager;
use crate::permissions::PermissionMode;
use crate::persona::load_persona;
use crate::session::Session;
use crate::skills::{SkillRegistry, parse_skill_invocation};
use crate::tools::{is_tool_allowed, tool_definitions};
use prism_context::model::DecisionScope;
use prism_context::store::{InboxFilters, MemoryFilters, Store as ContextStore};

const HUMAN_AGENT: &str = "human";

/// Helper struct providing tab completion for the REPL.
#[derive(rustyline::Helper, rustyline::Hinter, rustyline::Highlighter, rustyline::Validator)]
struct PrismHelper {
    commands: Vec<String>,
    personas: Vec<String>,
    threads: Arc<std::sync::Mutex<Vec<String>>>,
    models: Vec<String>,
    skills: Vec<String>,
    modes: Vec<String>,
}

fn filter(candidates: &[String], partial: &str) -> Vec<String> {
    candidates
        .iter()
        .filter(|c| c.starts_with(partial))
        .cloned()
        .collect()
}

impl Completer for PrismHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<String>)> {
        let input = &line[..pos];

        // Only complete lines starting with /
        if !input.starts_with('/') {
            return Ok((0, vec![]));
        }

        // If no space yet → Level 1: command name completion
        if !input.contains(' ') {
            return Ok((0, filter(&self.commands, input)));
        }

        // Level 2: argument completion
        let (cmd, partial_arg) = input.split_once(' ').unwrap();
        let partial = partial_arg.trim();
        let candidates = match cmd {
            "/persona" => filter(&self.personas, partial),
            "/mode" => filter(&self.modes, partial),
            "/thread" => filter(&self.threads.lock().unwrap(), partial),
            "/model" => filter(&self.models, partial),
            "/skill" => filter(&self.skills, partial),
            _ => vec![],
        };
        let arg_start = cmd.len() + 1;
        Ok((arg_start, candidates))
    }
}

enum MetaCommand {
    Clear,
    Compact,
    Help,
    Mode(Option<String>),
    Persona(Option<String>),
    Thread(Option<String>),
    Model(Option<String>),
    Who,
    Cost,
    Tools,
    Skills,
    Decide(String),
    Recall(Option<String>),
    Memory,
    AddDir(Option<String>),
}

impl MetaCommand {
    fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();
        match trimmed {
            "/clear" => Some(Self::Clear),
            "/compact" => Some(Self::Compact),
            "/help" => Some(Self::Help),
            "/mode" => Some(Self::Mode(None)),
            "/who" => Some(Self::Who),
            "/cost" => Some(Self::Cost),
            "/tools" => Some(Self::Tools),
            "/skills" => Some(Self::Skills),
            "/persona" => Some(Self::Persona(None)),
            "/thread" => Some(Self::Thread(None)),
            "/model" => Some(Self::Model(None)),
            "/recall" => Some(Self::Recall(None)),
            "/memory" => Some(Self::Memory),
            "/add-dir" => Some(Self::AddDir(None)),
            _ if trimmed.starts_with("/mode ") => Some(Self::Mode(parse_arg(trimmed, "/mode"))),
            _ if trimmed.starts_with("/persona ") => Some(Self::Persona(parse_arg(trimmed, "/persona"))),
            _ if trimmed.starts_with("/thread ") => Some(Self::Thread(parse_arg(trimmed, "/thread"))),
            _ if trimmed.starts_with("/model ") => Some(Self::Model(parse_arg(trimmed, "/model"))),
            _ if trimmed.starts_with("/decide ") => {
                Some(Self::Decide(parse_arg(trimmed, "/decide").unwrap_or_default()))
            }
            _ if trimmed.starts_with("/recall ") => Some(Self::Recall(parse_arg(trimmed, "/recall"))),
            _ if trimmed.starts_with("/add-dir ") => Some(Self::AddDir(parse_arg(trimmed, "/add-dir"))),
            _ => None,
        }
    }
}

fn parse_arg(input: &str, cmd: &str) -> Option<String> {
    input
        .strip_prefix(cmd)
        .and_then(|rest| rest.strip_prefix(' '))
        .map(|arg| arg.trim().to_string())
}

fn print_help() {
    eprintln!("Meta-commands:");
    eprintln!("  /help                Show this help");
    eprintln!("  /clear               Reset conversation (keeps session ID, rebuilds system prompt)");
    eprintln!("  /compact             Compress context window (LLM summarization or FIFO trim)");
    eprintln!("  /mode [<mode>]       Show or switch permission mode");
    eprintln!("                       Modes: default, accept-edits, plan, dont-ask, bypass");
    eprintln!("  /who                 Show session info (persona, thread, model, cost, turns)");
    eprintln!("  /cost                Show session cost breakdown");
    eprintln!("  /tools               List available tools and their allowed/denied status");
    eprintln!("  /skills              List loaded skills");
    eprintln!("  /persona [<name>]    Show or switch active persona");
    eprintln!("  /thread [<name>]     Show or switch active context thread");
    eprintln!("  /model [<name>]      Show or switch model (takes effect on next turn)");
    eprintln!("  /decide <title>      Record a decision in the current thread");
    eprintln!("  /recall [<thread>]   Recall thread context (memories, decisions, sessions)");
    eprintln!("  /memory              List memories for the current thread");
    eprintln!("  /add-dir [<path>]    Add a directory to the agent's context (or list current dirs)");
    eprintln!("  Ctrl+C               Exit");
    eprintln!();
    eprintln!("Skill invocations: /<skill-name> [args]");
    eprintln!("Just type a task to start an agent turn.");
}



pub async fn run_interactive(
    client: PrismClient,
    config: Config,
    session: Option<Session>,
    mcp_registry: Option<Arc<McpRegistry>>,
    memory: MemoryManager,
    skill_registry: SkillRegistry,
) -> Result<()> {
    // Fetch models before client is consumed by Agent.
    let models: Vec<String> = match client.list_models().await {
        Ok(resp) => resp.data.into_iter().map(|m| m.id).collect(),
        Err(_) => vec![],
    };

    let is_new_session = session.is_none();
    let mut agent = match session {
        Some(s) => {
            eprintln!(
                "[resume] episode {}  {} turns so far",
                &s.episode_id.to_string()[..8],
                s.turns
            );
            Agent::from_session(
                client,
                config,
                s,
                mcp_registry,
                memory,
                skill_registry.clone(),
            )
        }
        None => {
            // Placeholder task — cleared immediately on first user input
            Agent::new(
                client,
                config,
                "",
                mcp_registry,
                memory,
                skill_registry.clone(),
            )
        }
    };

    // Register HUMAN_AGENT as a first-class agent so it appears in uh agents and can receive messages.
    if let Some((store, ws_id)) = agent.store_context() {
        let _ = store.checkin(ws_id, HUMAN_AGENT, vec![], None).await;
    }

    // Install a ctrl-c handler that sets the shared flag. Fires during agent turns
    // (not during readline — rustyline catches Ctrl+C there via ReadlineError::Interrupted).
    let interrupt_flag = agent.interrupted.clone();
    tokio::spawn(async move {
        loop {
            let _ = tokio::signal::ctrl_c().await;
            interrupt_flag.store(true, Ordering::SeqCst);
        }
    });

    // Build completion data for PrismHelper.
    let mut commands: Vec<String> = vec![
        "/clear", "/compact", "/help", "/mode", "/persona", "/thread",
        "/model", "/who", "/cost", "/tools", "/skills", "/decide",
        "/recall", "/memory", "/add-dir",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    // Single pass: collect user-invocable skills into both lists at once.
    let mut skills: Vec<String> = Vec::new();
    for name in skill_registry.names() {
        if let Some(skill) = skill_registry.get(name) {
            if skill.user_invocable {
                commands.push(format!("/{name}"));
                skills.push(name.to_string());
            }
        }
    }

    let personas: Vec<String> = crate::persona::list_personas()
        .into_iter()
        .map(|(name, _)| name)
        .collect();

    let thread_names: Arc<std::sync::Mutex<Vec<String>>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    if let Some((store, ws_id)) = agent.store_context() {
        if let Ok(threads) = store.list_threads(ws_id, None).await {
            *thread_names.lock().unwrap() =
                threads.into_iter().map(|t| t.name).collect();
        }
    }

    // Derive mode names from the canonical PermissionMode enum.
    let modes: Vec<String> = [
        PermissionMode::Default,
        PermissionMode::AcceptEdits,
        PermissionMode::Plan,
        PermissionMode::DontAsk,
        PermissionMode::BypassPermissions,
    ]
    .iter()
    .map(|m| m.display_name().to_string())
    .collect();

    let helper = PrismHelper {
        commands,
        personas,
        threads: thread_names.clone(),
        models,
        skills,
        modes,
    };

    // Set up rustyline editor with persistent per-project history.
    let rl_config = rustyline::Config::builder()
        .max_history_size(1000)
        .unwrap()
        .auto_add_history(true)
        .edit_mode(EditMode::Emacs)
        .build();
    let mut rl_editor = rustyline::Editor::<PrismHelper, rustyline::history::FileHistory>::with_config(rl_config)?;
    rl_editor.set_helper(Some(helper));
    let history_path = repl_history_path(&std::env::current_dir().unwrap_or_default());
    if let Some(parent) = history_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = rl_editor.load_history(&history_path);
    let rl_editor: Arc<std::sync::Mutex<rustyline::Editor<PrismHelper, rustyline::history::FileHistory>>> =
        Arc::new(std::sync::Mutex::new(rl_editor));

    let renderer = Renderer::new();

    eprintln!("Interactive mode. Type /help for commands, Ctrl+C to exit.");
    eprintln!();

    let mut first_turn = is_new_session;

    loop {
        // Check interrupt at prompt — if set here, user wants to exit
        if agent.interrupted.load(Ordering::Relaxed) {
            eprintln!("\n[exit]");
            break;
        }

        // Set agent state to Idle while waiting for input
        agent.set_idle().await;

        // Show any background task completions before the prompt (non-consuming)
        for note in agent.poll_background_notifications() {
            renderer.notification_line(&note);
        }

        // Surface pending inbox entries (e.g. from subagents via ask_human)
        // and direct messages addressed to HUMAN_AGENT before each prompt.
        if let Some((store, ws_id)) = agent.store_context() {
            let filters = InboxFilters {
                unread_only: true,
                include_dismissed: false,
                limit: 10,
                entry_type: None,
            };
            let (inbox_result, msgs_result) = tokio::join!(
                store.list_inbox_entries(ws_id, filters),
                store.list_messages(ws_id, HUMAN_AGENT, true),
            );
            if let Ok(entries) = inbox_result {
                let items: Vec<(String, String)> = entries
                    .iter()
                    .map(|e| {
                        let from = e.source_agent.as_deref().unwrap_or("agent");
                        (format!("{}/{}", e.severity, from), e.body.clone())
                    })
                    .collect();
                renderer.inbox_box("inbox", &items);
                for e in &entries {
                    let _ = store.mark_inbox_read(ws_id, e.id).await;
                }
            }
            if let Ok(msgs) = msgs_result {
                let items: Vec<(String, String)> = msgs
                    .iter()
                    .map(|m| (m.from_agent.clone(), m.content.clone()))
                    .collect();
                renderer.inbox_box("messages", &items);
                if !msgs.is_empty() {
                    let _ = store.mark_messages_read(ws_id, HUMAN_AGENT).await;
                }
            }
        }

        let prompt = agent.build_prompt();
        let rl = rl_editor.clone();
        let readline_result: Result<String, rustyline::error::ReadlineError> =
            tokio::task::spawn_blocking(move || {
                let mut editor = rl.lock().unwrap();
                editor.readline(&prompt)
            })
            .await
            .unwrap_or(Err(rustyline::error::ReadlineError::Eof));

        let line: String = match readline_result {
            Ok(line) => line,
            Err(rustyline::error::ReadlineError::Interrupted) => {
                // Ctrl+C at prompt — exit
                eprintln!("\n[exit]");
                break;
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                // EOF (piped input exhausted)
                break;
            }
            Err(_) => break,
        };

        // Multiline input: trailing `\` continues to next line.
        let input: String = {
            let first = line.trim_end();
            if first.ends_with('\\') {
                let mut buf = first[..first.len() - 1].to_string();
                loop {
                    let rl2 = rl_editor.clone();
                    let cont_result: Result<String, rustyline::error::ReadlineError> =
                        tokio::task::spawn_blocking(move || {
                            let mut ed = rl2.lock().unwrap();
                            ed.readline("... ")
                        })
                        .await
                        .unwrap_or(Err(rustyline::error::ReadlineError::Eof));
                    match cont_result {
                        Ok(cont_line) => {
                            let trimmed = cont_line.trim_end();
                            if trimmed.ends_with('\\') {
                                buf.push('\n');
                                buf.push_str(&trimmed[..trimmed.len() - 1]);
                            } else {
                                buf.push('\n');
                                buf.push_str(trimmed.trim_start());
                                break;
                            }
                        }
                        Err(_) => break, // Ctrl+C or EOF during continuation: submit what we have
                    }
                }
                buf
            } else {
                first.to_string()
            }
        };
        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        // Dispatch meta-commands first
        if let Some(cmd) = MetaCommand::parse(input) {
            match cmd {
                MetaCommand::Help => print_help(),
                MetaCommand::Clear => {
                    // ANSI clear screen
                    eprint!("\x1b[2J\x1b[H");
                    agent.clear_conversation().await;
                    first_turn = true;
                    eprintln!("[clear] conversation reset");
                }
                MetaCommand::Compact => {
                    agent.compact().await;
                }
                MetaCommand::Mode(None) => {
                    let name = agent.permission_mode().display_name();
                    eprintln!("[mode] current: {name}");
                    eprintln!("       available: default, accept-edits, plan, dont-ask, bypass");
                }
                MetaCommand::Mode(Some(arg)) => {
                    if let Ok(mode) = PermissionMode::from_str(&arg, true) {
                        agent.set_permission_mode(mode);
                        eprintln!("[mode] switched to: {arg}");
                    } else {
                        eprintln!("[error] unknown mode '{arg}'. Available: default, accept-edits, plan, dont-ask, bypass");
                    }
                }
                MetaCommand::Persona(None) => {
                    let name = agent.config.session.persona.as_deref().unwrap_or("(none)");
                    eprintln!("[persona] current: {name}");
                }
                MetaCommand::Persona(Some(name)) => {
                    match load_persona(&name) {
                        Ok(persona) => {
                            persona.apply(&mut agent.config);
                            // apply() may set config.extensions.permission_mode — sync the gate
                            if let Some(mode) = agent.config.extensions.permission_mode {
                                agent.set_permission_mode(mode);
                            }
                            agent.config.session.persona = Some(name.clone());
                            eprintln!("[persona] switched to: {name}");
                        }
                        Err(e) => eprintln!("[error] {e}"),
                    }
                }
                MetaCommand::Thread(None) => {
                    let name = agent.current_thread.as_deref().unwrap_or("(none)");
                    eprintln!("[thread] current: {name}");
                }
                MetaCommand::Thread(Some(name)) => {
                    if let Some((store, ws_id)) = agent.store_context() {
                        match store.get_thread(ws_id, &name).await {
                            Ok(_) => {}
                            Err(prism_context::error::Error::NotFound(_)) => {
                                let _ = store.create_thread(ws_id, &name, "", vec![]).await;
                            }
                            Err(e) => {
                                eprintln!("[error] {e}");
                            }
                        }
                        // Refresh completion cache after thread changes
                        if let Ok(threads) = store.list_threads(ws_id, None).await {
                            *thread_names.lock().unwrap() =
                                threads.into_iter().map(|t| t.name).collect();
                        }
                    }
                    agent.current_thread = Some(name.clone());
                    eprintln!("[thread] switched to: {name}");
                }
                MetaCommand::Model(None) => {
                    eprintln!("[model] current: {}", agent.config.model.model);
                }
                MetaCommand::Model(Some(name)) => {
                    agent.config.model.model = name.clone();
                    eprintln!("[model] switched to: {name} (takes effect on next turn)");
                }
                MetaCommand::Who => {
                    let persona = agent.config.session.persona.as_deref().unwrap_or("(none)");
                    let thread = agent.current_thread.as_deref().unwrap_or("(none)");
                    let model = &agent.config.model.model;
                    let mode_name = agent.permission_mode().display_name();
                    let cost = agent.session.total_cost_usd;
                    let turns = agent.session.turns;
                    let episode = &agent.session.episode_id.to_string()[..8];
                    eprintln!("[who]");
                    eprintln!("  persona: {persona}");
                    eprintln!("  thread:  {thread}");
                    eprintln!("  model:   {model}");
                    eprintln!("  mode:    {mode_name}");
                    eprintln!("  cost:    ${cost:.4}");
                    eprintln!("  turns:   {turns}");
                    eprintln!("  episode: {episode}");
                }
                MetaCommand::Cost => {
                    let total = agent.session.total_cost_usd;
                    let cap = agent.config.model.max_cost_usd;
                    let prompt_tokens = agent.session.total_prompt_tokens;
                    let completion_tokens = agent.session.total_completion_tokens;
                    eprintln!("[cost]");
                    eprintln!("  session total: ${total:.4}");
                    if let Some(c) = cap {
                        let pct = 100.0 * total / c;
                        eprintln!("  cost cap:      ${c:.4}");
                        eprintln!("  % used:        {pct:.1}%");
                    } else {
                        eprintln!("  cost cap:      unlimited");
                    }
                    eprintln!("  prompt tokens:     {prompt_tokens}");
                    eprintln!("  completion tokens: {completion_tokens}");
                }
                MetaCommand::Tools => {
                    let print_tools = |tools: &[prism_types::Tool], config: &Config| {
                        for tool in tools {
                            let name = &tool.function.name;
                            let status = if is_tool_allowed(name, config) { "allow" } else { "deny" };
                            eprintln!("    [{status}] {name}");
                        }
                    };
                    eprintln!("[tools] sandbox: {:?}", agent.config.session.sandbox_mode);
                    eprintln!("  builtin:");
                    print_tools(&tool_definitions(), &agent.config);
                    if let Some(reg) = &agent.mcp_registry {
                        let mcp = reg.tool_definitions();
                        if !mcp.is_empty() {
                            eprintln!("  mcp:");
                            print_tools(mcp, &agent.config);
                        }
                    }
                }
                MetaCommand::Skills => {
                    let names = agent.skill_registry.names();
                    if names.is_empty() {
                        eprintln!("[skills] (no skills loaded)");
                    } else {
                        eprintln!("[skills]");
                        for name in names {
                            eprintln!("  /{name}");
                        }
                    }
                }
                MetaCommand::Decide(title) => {
                    if let Some((store, ws_id)) = agent.store_context() {
                        let thread_id = if let Some(ref t) = agent.current_thread {
                            store.get_thread(ws_id, t).await.ok().map(|th| th.id)
                        } else {
                            None
                        };
                        match store
                            .save_decision(ws_id, &title, "", thread_id, vec![], DecisionScope::Thread)
                            .await
                        {
                            Ok(d) => eprintln!("[decide] recorded: {} (id: {})", title, &d.id.to_string()[..8]),
                            Err(e) => eprintln!("[error] {e}"),
                        }
                    } else {
                        eprintln!("[error] no context store available");
                    }
                }
                MetaCommand::Recall(arg) => {
                    let thread_name = arg.or_else(|| agent.current_thread.clone());
                    if let Some(name) = thread_name {
                        if let Some((store, ws_id)) = agent.store_context() {
                            match store.recall_thread(ws_id, &name).await {
                                Ok(ctx) => {
                                    eprintln!("[recall] thread: {}", ctx.thread.name);
                                    if !ctx.memories.is_empty() {
                                        eprintln!("  memories ({}):", ctx.memories.len());
                                        for m in &ctx.memories {
                                            eprintln!("    {} = {}", m.key, m.value);
                                        }
                                    }
                                    if !ctx.decisions.is_empty() {
                                        eprintln!("  decisions ({}):", ctx.decisions.len());
                                        for d in &ctx.decisions {
                                            eprintln!("    - {}", d.title);
                                        }
                                    }
                                    if !ctx.recent_sessions.is_empty() {
                                        eprintln!("  recent sessions ({}):", ctx.recent_sessions.len());
                                        for s in &ctx.recent_sessions {
                                            let summary = if s.summary.is_empty() { "(no summary)" } else { &s.summary };
                                            eprintln!("    - {summary}");
                                        }
                                    }
                                }
                                Err(e) => eprintln!("[error] {e}"),
                            }
                        } else {
                            eprintln!("[error] no context store available");
                        }
                    } else {
                        eprintln!("[error] no thread set — use /thread <name> or /recall <thread>");
                    }
                }
                MetaCommand::Memory => {
                    if let Some((store, ws_id)) = agent.store_context() {
                        let filters = MemoryFilters {
                            thread_name: agent.current_thread.clone(),
                            ..Default::default()
                        };
                        match store.load_memories(ws_id, filters).await {
                            Ok(memories) => {
                                if memories.is_empty() {
                                    eprintln!("[memory] (none)");
                                } else {
                                    eprintln!("[memory]");
                                    for m in &memories {
                                        eprintln!("  {} = {}", m.key, m.value);
                                    }
                                }
                            }
                            Err(e) => eprintln!("[error] {e}"),
                        }
                    } else {
                        eprintln!("[error] no context store available");
                    }
                }
                MetaCommand::AddDir(None) => {
                    if agent.additional_dirs.is_empty() {
                        eprintln!("[add-dir] (no extra directories)");
                    } else {
                        eprintln!("[add-dir] active directories:");
                        for dir in &agent.additional_dirs {
                            eprintln!("  {}", dir.display());
                        }
                    }
                }
                MetaCommand::AddDir(Some(raw)) => {
                    let expanded = shellexpand::tilde(&raw).into_owned();
                    let path = std::path::PathBuf::from(&expanded);
                    let canonical = std::fs::canonicalize(&path);
                    match canonical {
                        Ok(abs) if abs.is_dir() => {
                            if agent.additional_dirs.contains(&abs) {
                                eprintln!("[add-dir] already added: {}", abs.display());
                            } else {
                                agent.additional_dirs.push(abs.clone());
                                eprintln!("[add-dir] added: {}", abs.display());
                                eprintln!("          agent can now read files from this directory");
                            }
                        }
                        Ok(abs) => {
                            eprintln!("[error] not a directory: {}", abs.display());
                        }
                        Err(e) => {
                            eprintln!("[error] {raw}: {e}");
                        }
                    }
                }
            }
            continue;
        }

        // Expand skill invocations (e.g. /commit "message")
        let expanded: String;
        let task_str = if let Some((skill_name, skill_args)) = parse_skill_invocation(input) {
            match skill_registry.get(skill_name) {
                Some(skill) if skill.user_invocable => {
                    eprintln!("[skill] expanding /{skill_name}");
                    expanded = skill.expand(skill_args);
                    &expanded
                }
                Some(_) => {
                    eprintln!("[error] skill '{skill_name}' is not user-invocable");
                    continue;
                }
                None => {
                    eprintln!(
                        "[error] unknown skill: '{skill_name}'. Available: {:?}",
                        skill_registry.names()
                    );
                    continue;
                }
            }
        } else {
            input
        };

        // Record the human's task as a Plan in uglyhat — fire-and-forget.
        if let Some((store, ws_id)) = agent.store_context() {
            let intent = task_str.to_string();
            tokio::spawn(async move {
                let _ = store.create_plan(ws_id, &intent).await;
            });
        }

        // Reset interrupt so Ctrl+C during this turn only interrupts the turn
        agent.interrupted.store(false, Ordering::SeqCst);

        let pre_cost = agent.session.total_cost_usd;
        let pre_turns = agent.session.turns;

        let result = if first_turn {
            first_turn = false;
            agent.run(task_str).await
        } else {
            agent.resume(task_str).await
        };

        if let Err(e) = result {
            eprintln!("[error] {e:#}");
        }

        if agent.session.turns > pre_turns {
            renderer.turn_separator(agent.session.turns, agent.session.total_cost_usd - pre_cost);
        }

        // After each turn, ensure interrupt flag is clear so next prompt reads correctly
        agent.interrupted.store(false, Ordering::SeqCst);

        // Save history after each turn so crashes only lose the last unsaved command
        if let Ok(mut editor) = rl_editor.lock() {
            let _ = editor.save_history(&history_path);
        }
    }

    // Exit cleanup: final history flush, checkout, session summary, session save.
    if let Ok(mut editor) = rl_editor.lock() {
        let _ = editor.save_history(&history_path);
    }

    let model = agent.session.model.clone();
    let turns = agent.session.turns;
    let tokens_in = agent.session.total_prompt_tokens;
    let tokens_out = agent.session.total_completion_tokens;
    let cost = agent.session.total_cost_usd;
    let episode_id = agent.session.episode_id.to_string();

    if let Some((store, ws_id)) = agent.store_context() {
        let summary = format!("{turns} turns, ${cost:.4}");
        let _ = store
            .checkout(ws_id, HUMAN_AGENT, &summary, vec![], vec![], vec![])
            .await;
    }

    renderer.session_summary(&model, turns, tokens_in, tokens_out, cost, &episode_id);

    let _ = agent.session.save(&agent.config.session.sessions_dir);

    Ok(())
}
