use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global, Task};

const MAX_OUTPUT_LINES: usize = 200;

#[derive(Debug, Clone)]
pub enum RunningAgentsEvent {
    AgentExited { agent_name: String },
}

/// Ring buffer of output lines captured from a running agent process.
#[derive(Clone)]
pub struct AgentOutput(Arc<Mutex<VecDeque<String>>>);

impl AgentOutput {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(VecDeque::with_capacity(
            MAX_OUTPUT_LINES,
        ))))
    }

    pub fn new_empty() -> Self {
        Self::new()
    }

    pub fn push(&self, line: String) {
        let mut buf = self.0.lock().unwrap();
        if buf.len() >= MAX_OUTPUT_LINES {
            buf.pop_front();
        }
        buf.push_back(line);
    }

    pub fn lines(&self) -> Vec<String> {
        self.0.lock().unwrap().iter().cloned().collect()
    }
}

pub struct RunningAgent {
    pub agent_name: String,
    pub output: AgentOutput,
    pub is_running: bool,
}

/// GPUI entity that tracks all agents currently spawned from this IDE session.
pub struct RunningAgents {
    pub processes: HashMap<String, RunningAgent>,
    _reader_tasks: Vec<Task<()>>,
}

pub struct RunningAgentsGlobal(pub Entity<RunningAgents>);
impl Global for RunningAgentsGlobal {}

impl EventEmitter<RunningAgentsEvent> for RunningAgents {}

impl RunningAgents {
    pub fn init_global(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| RunningAgents {
            processes: HashMap::new(),
            _reader_tasks: Vec::new(),
        });
        cx.set_global(RunningAgentsGlobal(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<RunningAgentsGlobal>().map(|g| g.0.clone())
    }

    /// Register a new agent process and start streaming its output into the ring buffer.
    pub fn register(
        &mut self,
        agent_name: String,
        output_receiver: tokio::sync::mpsc::UnboundedReceiver<String>,
        _entity: gpui::WeakEntity<RunningAgents>,
        cx: &mut Context<RunningAgents>,
    ) {
        let output = AgentOutput::new();
        self.processes.insert(
            agent_name.clone(),
            RunningAgent {
                agent_name: agent_name.clone(),
                output: output.clone(),
                is_running: true,
            },
        );

        // Drain the output channel in a background task and notify GPUI on each line.
        let task = cx.spawn(async move |this, cx| {
            let mut receiver = output_receiver;
            while let Some(line) = receiver.recv().await {
                output.push(line);
                this.update(cx, |_, cx| cx.notify()).ok();
            }
            // Channel closed — process exited.
            this.update(cx, |ra, cx| {
                if let Some(proc) = ra.processes.get_mut(&agent_name) {
                    proc.is_running = false;
                }
                cx.emit(RunningAgentsEvent::AgentExited { agent_name: agent_name.clone() });
                cx.notify();
            })
            .ok();
        });
        self._reader_tasks.push(task);
    }

    pub fn was_spawned(&self, agent_name: &str) -> bool {
        self.processes.contains_key(agent_name)
    }

    /// Returns true if the agent was spawned in this session and has since exited.
    pub fn is_completed(&self, agent_name: &str) -> bool {
        self.was_spawned(agent_name) && !self.is_running(agent_name)
    }

    pub fn is_running(&self, agent_name: &str) -> bool {
        self.processes
            .get(agent_name)
            .map(|p| p.is_running)
            .unwrap_or(false)
    }

    pub fn output_lines(&self, agent_name: &str) -> Vec<String> {
        self.processes
            .get(agent_name)
            .map(|p| p.output.lines())
            .unwrap_or_default()
    }
}
