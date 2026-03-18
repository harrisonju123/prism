use std::collections::HashMap;

use prism_context::model::{AgentState, AgentStatus};

use crate::activity_bus::AgentActivityBusInner;
use super::characters::{BubbleKind, CharState};

// ── mutations ─────────────────────────────────────────────────────────────────

/// Changes to apply to `OfficeState` after syncing agent data.
#[derive(Debug)]
pub enum OfficeMutation {
    SpawnCharacter {
        agent_name: String,
        palette: usize,
    },
    DespawnCharacter {
        agent_name: String,
    },
    SetState {
        agent_name: String,
        char_state: CharState,
        status_text: Option<String>,
    },
    ShowBubble {
        agent_name: String,
        kind: BubbleKind,
    },
    ClearBubble {
        agent_name: String,
    },
}

// ── bridge ────────────────────────────────────────────────────────────────────

/// Maps agent names → character IDs and produces `OfficeMutation`s on each sync.
pub struct AgentBridge {
    /// Agent name → character id mapping.
    pub agent_to_char: HashMap<String, usize>,
    /// Previous bubble state per agent so we don't re-emit on every tick.
    prev_bubble: HashMap<String, Option<BubbleKind>>,
    /// Previous char state per agent.
    prev_state: HashMap<String, CharState>,
    next_palette: usize,
    next_char_id: usize,
}

impl AgentBridge {
    pub fn new() -> Self {
        Self {
            agent_to_char: HashMap::new(),
            prev_bubble: HashMap::new(),
            prev_state: HashMap::new(),
            next_palette: 0,
            next_char_id: 0,
        }
    }

    /// Produce the list of mutations required to reconcile agent roster with office state.
    ///
    /// `local_agent_name` is the name of this IDE's agent (from `PRISM_AGENT_NAME`).
    pub fn sync(
        &mut self,
        agents: &[AgentStatus],
        activity: Option<&AgentActivityBusInner>,
        local_agent_name: Option<&str>,
    ) -> Vec<OfficeMutation> {
        let mut mutations = Vec::new();

        // Determine which agents are alive (session open, not dead).
        let live_names: std::collections::HashSet<String> = agents
            .iter()
            .filter(|a| a.session_open && a.state != AgentState::Dead)
            .map(|a| a.name.clone())
            .collect();

        // Despawn characters whose agents are gone.
        let to_despawn: Vec<String> = self
            .agent_to_char
            .keys()
            .filter(|name| !live_names.contains(*name))
            .cloned()
            .collect();
        for name in to_despawn {
            self.agent_to_char.remove(&name);
            self.prev_bubble.remove(&name);
            self.prev_state.remove(&name);
            mutations.push(OfficeMutation::DespawnCharacter {
                agent_name: name,
            });
        }

        // Spawn new agents, update states for existing ones.
        for agent in agents {
            if !agent.session_open || agent.state == AgentState::Dead {
                continue;
            }

            // Spawn if not yet tracked.
            if !self.agent_to_char.contains_key(&agent.name) {
                let id = self.next_char_id;
                self.next_char_id += 1;
                let palette = self.next_palette % 6;
                self.next_palette += 1;
                self.agent_to_char.insert(agent.name.clone(), id);
                mutations.push(OfficeMutation::SpawnCharacter {
                    agent_name: agent.name.clone(),
                    palette,
                });
            }

            // Determine desired char state and status text.
            let (char_state, status_text, bubble) =
                if local_agent_name == Some(agent.name.as_str()) {
                    // Local agent: use high-resolution ActivityBus data.
                    self.state_from_activity(activity)
                } else {
                    // Remote agent: use coarse AgentStatus.
                    self.state_from_agent_status(&agent.state)
                };

            // Emit SetState if changed.
            if self.prev_state.get(&agent.name) != Some(&char_state) {
                self.prev_state.insert(agent.name.clone(), char_state);
                mutations.push(OfficeMutation::SetState {
                    agent_name: agent.name.clone(),
                    char_state,
                    status_text,
                });
            }

            // Emit bubble mutation if changed.
            let prev = self.prev_bubble.get(&agent.name).copied().flatten();
            match (prev, bubble) {
                (None, Some(kind)) => {
                    self.prev_bubble.insert(agent.name.clone(), Some(kind));
                    mutations.push(OfficeMutation::ShowBubble {
                        agent_name: agent.name.clone(),
                        kind,
                    });
                }
                (Some(_), None) => {
                    self.prev_bubble.insert(agent.name.clone(), None);
                    mutations.push(OfficeMutation::ClearBubble {
                        agent_name: agent.name.clone(),
                    });
                }
                _ => {}
            }
        }

        mutations
    }

    fn state_from_agent_status(
        &self,
        status: &AgentState,
    ) -> (CharState, Option<String>, Option<BubbleKind>) {
        match status {
            AgentState::Working => (CharState::Type, Some("Working…".into()), None),
            AgentState::Idle => (CharState::Idle, None, None),
            AgentState::AwaitingReview => (
                CharState::Wait,
                Some("Waiting for review…".into()),
                Some(BubbleKind::Waiting),
            ),
            AgentState::Blocked => (
                CharState::Wait,
                Some("Blocked".into()),
                Some(BubbleKind::Waiting),
            ),
            AgentState::Dead => (CharState::Idle, None, None),
        }
    }

    fn state_from_activity(
        &self,
        activity: Option<&AgentActivityBusInner>,
    ) -> (CharState, Option<String>, Option<BubbleKind>) {
        let Some(bus) = activity else {
            return (CharState::Idle, None, None);
        };

        if bus.waiting_for_approval {
            return (
                CharState::Wait,
                Some("Waiting for approval…".into()),
                Some(BubbleKind::Permission),
            );
        }

        if bus.is_generating {
            let status_text = if let (Some(tool), Some(file)) =
                (&bus.current_tool, &bus.current_file)
            {
                Some(format_tool_status(tool, Some(file)))
            } else if let Some(tool) = &bus.current_tool {
                Some(format_tool_status(tool, None))
            } else {
                Some("Working…".into())
            };
            return (CharState::Type, status_text, None);
        }

        (CharState::Idle, None, None)
    }
}

/// Map a raw tool name + optional file to a human-readable status string.
pub fn format_tool_status(tool: &str, file: Option<&str>) -> String {
    let short_file = file.map(|f| {
        // Show only the last component of the path.
        std::path::Path::new(f)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(f)
    });

    match tool {
        "read_file" | "grep" | "find_path" | "codebase_search" | "glob" => {
            if let Some(f) = short_file {
                format!("Reading {f}")
            } else {
                "Reading…".into()
            }
        }
        "edit_file" | "streaming_edit_file" | "write_file" | "create_file" => {
            if let Some(f) = short_file {
                format!("Editing {f}")
            } else {
                "Editing…".into()
            }
        }
        "terminal" | "bash" | "run_command" => "Running command…".into(),
        "spawn_agent" | "spawn_subagent" => "Spawning subagent…".into(),
        "ask_human" | "escalate_decision" => "Waiting for input…".into(),
        "web_search" | "web_fetch" => "Searching web…".into(),
        _ => "Working…".into(),
    }
}
