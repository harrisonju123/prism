use anyhow::Result;
use prism_client::PrismClient;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::agent::Agent;
use crate::config::Config;
use crate::mcp::McpRegistry;
use crate::memory::MemoryManager;
use crate::session::Session;
use crate::skills::{SkillRegistry, parse_skill_invocation};

enum MetaCommand {
    Clear,
    Compact,
    Help,
}

impl MetaCommand {
    fn parse(input: &str) -> Option<Self> {
        match input.trim() {
            "/clear" => Some(Self::Clear),
            "/compact" => Some(Self::Compact),
            "/help" => Some(Self::Help),
            _ => None,
        }
    }
}

fn print_help() {
    eprintln!("Meta-commands:");
    eprintln!("  /help     Show this help");
    eprintln!("  /clear    Reset conversation (keeps session ID, rebuilds system prompt)");
    eprintln!("  /compact  Compress context window (LLM summarization or FIFO trim)");
    eprintln!("  Ctrl+C    Exit");
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

    // Register "human" as a first-class agent so it appears in uh agents and can receive messages.
    if let Some((store, ws_id)) = agent.store_context() {
        use prism_context::store::Store as _;
        let _ = store.checkin(ws_id, "human", vec![], None).await;
    }

    // Install a ctrl-c handler that sets the shared flag. In REPL mode we want Ctrl+C at the
    // prompt to exit, and Ctrl+C during a turn to interrupt just that turn.
    let interrupt_flag = agent.interrupted.clone();
    tokio::spawn(async move {
        loop {
            let _ = tokio::signal::ctrl_c().await;
            interrupt_flag.store(true, Ordering::SeqCst);
        }
    });

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
            eprintln!("{note}");
        }

        // Surface pending inbox entries (e.g. from subagents via ask_human)
        // and direct messages addressed to "human" before each prompt.
        if let Some((store, ws_id)) = agent.store_context() {
            use prism_context::store::{InboxFilters, Store};
            let filters = InboxFilters {
                unread_only: true,
                include_dismissed: false,
                limit: 10,
                entry_type: None,
            };
            if let Ok(entries) = store.list_inbox_entries(ws_id, filters).await {
                for e in &entries {
                    let from = e.source_agent.as_deref().unwrap_or("agent");
                    eprintln!("[inbox/{}/{}] {}", e.severity, from, e.body);
                    let _ = store.mark_inbox_read(ws_id, e.id).await;
                }
            }
            if let Ok(msgs) = store.list_messages(ws_id, "human", true).await {
                for m in &msgs {
                    eprintln!("[message from {}] {}", m.from_agent, m.content);
                }
                if !msgs.is_empty() {
                    let _ = store.mark_messages_read(ws_id, "human").await;
                }
            }
        }

        eprint!("> ");
        let _ = std::io::Write::flush(&mut std::io::stderr());

        // Read a line from stdin in a blocking thread so we can select! with ctrl-c
        let line = tokio::task::spawn_blocking(|| {
            let mut buf = String::new();
            match std::io::stdin().read_line(&mut buf) {
                Ok(0) => None, // EOF
                Ok(_) => Some(buf),
                Err(_) => None,
            }
        })
        .await
        .unwrap_or(None);

        let Some(line) = line else {
            // EOF (piped input exhausted)
            break;
        };

        let input = line.trim();
        if input.is_empty() {
            continue;
        }

        // Check interrupt after read (Ctrl+C during readline)
        if agent.interrupted.load(Ordering::Relaxed) {
            eprintln!("\n[exit]");
            break;
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
                use prism_context::store::Store;
                let _ = store.create_plan(ws_id, &intent).await;
            });
        }

        // Reset interrupt so Ctrl+C during this turn only interrupts the turn
        agent.interrupted.store(false, Ordering::SeqCst);

        let result = if first_turn {
            first_turn = false;
            agent.run(task_str).await
        } else {
            agent.resume(task_str).await
        };

        if let Err(e) = result {
            eprintln!("[error] {e:#}");
        }

        // After each turn, ensure interrupt flag is clear so next prompt reads correctly
        agent.interrupted.store(false, Ordering::SeqCst);
    }

    Ok(())
}
