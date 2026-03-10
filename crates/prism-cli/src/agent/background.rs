use std::future::Future;
use std::time::Instant;

use tokio::sync::mpsc;

use super::spawn::AgentResult;

#[derive(Debug)]
pub struct BackgroundTaskInfo {
    pub task_id: String,
    pub description: String,
    pub started_at: Instant,
}

#[derive(Debug)]
pub struct CompletedTask {
    pub task_id: String,
    pub description: String,
    pub result: AgentResult,
    pub elapsed_secs: f64,
}

pub struct BackgroundTaskManager {
    active: Vec<BackgroundTaskInfo>,
    rx: mpsc::UnboundedReceiver<CompletedTask>,
    tx: mpsc::UnboundedSender<CompletedTask>,
    /// Tasks drained from channel but not yet injected into the conversation.
    pending: Vec<CompletedTask>,
}

const MAX_BACKGROUND_TASKS: usize = 5;

impl Default for BackgroundTaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundTaskManager {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            active: Vec::new(),
            rx,
            tx,
            pending: Vec::new(),
        }
    }

    /// Spawn a background task. Registers it, runs the future via tokio::spawn,
    /// and sends the result back through the internal channel.
    pub fn spawn_task<F>(
        &mut self,
        task_id: String,
        description: String,
        fut: F,
    ) -> Result<(), String>
    where
        F: Future<Output = AgentResult> + Send + 'static,
    {
        if self.active.len() >= MAX_BACKGROUND_TASKS {
            return Err(format!(
                "max background tasks ({MAX_BACKGROUND_TASKS}) reached — wait for one to complete"
            ));
        }
        self.active.push(BackgroundTaskInfo {
            task_id: task_id.clone(),
            description: description.clone(),
            started_at: Instant::now(),
        });
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let t0 = Instant::now();
            let result = fut.await;
            let _ = tx.send(CompletedTask {
                task_id,
                description,
                result,
                elapsed_secs: t0.elapsed().as_secs_f64(),
            });
        });
        Ok(())
    }

    /// Drain channel into `pending` buffer. Returns newly arrived completions
    /// for rendering (REPL notifications). Does NOT consume them — call
    /// `take_pending()` to consume for LLM injection.
    pub fn poll_completed(&mut self) -> Vec<&CompletedTask> {
        let start = self.pending.len();
        while let Ok(task) = self.rx.try_recv() {
            self.active.retain(|a| a.task_id != task.task_id);
            self.pending.push(task);
        }
        self.pending[start..].iter().collect()
    }

    /// Take all pending completed tasks for injection into the conversation.
    /// Drains the channel first, then returns and clears the buffer.
    pub fn take_pending(&mut self) -> Vec<CompletedTask> {
        // Drain any new arrivals into the buffer first
        while let Ok(task) = self.rx.try_recv() {
            self.active.retain(|a| a.task_id != task.task_id);
            self.pending.push(task);
        }
        std::mem::take(&mut self.pending)
    }

    pub fn active_tasks(&self) -> &[BackgroundTaskInfo] {
        &self.active
    }
}
