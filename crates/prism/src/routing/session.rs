use crate::types::TaskType;
use std::collections::{HashMap, VecDeque};
use std::time::Instant;

/// Session phases for routing consistency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionPhase {
    Planning,
    Implementing,
    Iterating,
    Finishing,
}

impl SessionPhase {
    /// Representative task type for each phase.
    pub fn representative_task(&self) -> TaskType {
        match self {
            Self::Planning => TaskType::Architecture,
            Self::Implementing => TaskType::CodeGeneration,
            Self::Iterating => TaskType::Debugging,
            Self::Finishing => TaskType::Documentation,
        }
    }
}

/// Map task types to session phases.
fn task_to_phase(task: TaskType) -> SessionPhase {
    match task {
        TaskType::Architecture | TaskType::Reasoning => SessionPhase::Planning,
        TaskType::CodeGeneration | TaskType::CodeReview | TaskType::Refactoring => {
            SessionPhase::Implementing
        }
        TaskType::Debugging | TaskType::Testing => SessionPhase::Iterating,
        TaskType::Documentation | TaskType::Summarization => SessionPhase::Finishing,
        _ => SessionPhase::Implementing, // default
    }
}

const MAX_WINDOW: usize = 10;
const MAX_SESSION_AGE_SECS: u64 = 3600;

struct SessionState {
    tasks: VecDeque<TaskType>,
    last_seen: Instant,
}

pub struct SessionTracker {
    sessions: HashMap<String, SessionState>,
}

impl SessionTracker {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Record a task type for a session and return the detected phase.
    pub fn record(&mut self, session_id: &str, task_type: TaskType) -> SessionPhase {
        let state = self
            .sessions
            .entry(session_id.to_string())
            .or_insert_with(|| SessionState {
                tasks: VecDeque::new(),
                last_seen: Instant::now(),
            });

        state.last_seen = Instant::now();
        state.tasks.push_back(task_type);
        if state.tasks.len() > MAX_WINDOW {
            state.tasks.pop_front();
        }

        self.detect_phase(session_id)
    }

    /// Detect session phase via majority vote over the sliding window.
    fn detect_phase(&self, session_id: &str) -> SessionPhase {
        let state = match self.sessions.get(session_id) {
            Some(s) => s,
            None => return SessionPhase::Implementing,
        };

        let mut counts: HashMap<SessionPhase, usize> = HashMap::new();
        for task in &state.tasks {
            *counts.entry(task_to_phase(*task)).or_default() += 1;
        }

        counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(phase, _)| phase)
            .unwrap_or(SessionPhase::Implementing)
    }

    /// Remove sessions older than MAX_SESSION_AGE_SECS.
    pub fn prune(&mut self) {
        self.sessions
            .retain(|_, state| state.last_seen.elapsed().as_secs() < MAX_SESSION_AGE_SECS);
    }
}
