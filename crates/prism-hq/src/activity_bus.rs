use gpui::{App, AppContext as _, Context, Entity, EventEmitter, Global};

/// GPUI Global — written by agent_ui when a thread is active,
/// read by prism-hq status indicator for live activity display.
pub struct AgentActivityBus(pub Entity<AgentActivityBusInner>);

impl Global for AgentActivityBus {}

pub struct AgentActivityBusInner {
    pub is_generating: bool,
    /// Programmatic tool name (e.g. "edit_file", "bash")
    pub current_tool: Option<String>,
    /// File path currently being edited/read
    pub current_file: Option<String>,
    /// Files edited so far this turn (accumulates across tool calls)
    pub files_edited: Vec<String>,
    /// Turn index within the session
    pub turn_index: usize,
    /// Title of the active thread
    pub thread_title: Option<String>,
    /// Whether the agent is waiting for tool authorization approval
    pub waiting_for_approval: bool,
}

impl EventEmitter<()> for AgentActivityBusInner {}

impl AgentActivityBusInner {
    pub fn update_tool(
        &mut self,
        tool: impl Into<String>,
        file: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.current_tool = Some(tool.into());
        self.current_file = file;
        cx.notify();
    }

    pub fn set_generating(&mut self, generating: bool, cx: &mut Context<Self>) {
        self.is_generating = generating;
        if !generating {
            self.current_tool = None;
            self.current_file = None;
            self.waiting_for_approval = false;
        }
        cx.notify();
    }

    pub fn add_edited_file(&mut self, path: impl Into<String>, cx: &mut Context<Self>) {
        let path = path.into();
        if !self.files_edited.contains(&path) {
            self.files_edited.push(path);
            cx.notify();
        }
    }

    pub fn set_waiting_for_approval(&mut self, waiting: bool, cx: &mut Context<Self>) {
        if self.waiting_for_approval != waiting {
            self.waiting_for_approval = waiting;
            cx.notify();
        }
    }

    pub fn set_thread_title(&mut self, title: Option<String>, cx: &mut Context<Self>) {
        self.thread_title = title;
        cx.notify();
    }

    /// Start a new thread — resets all fields and sets the title and generating state in one notify.
    pub fn start_thread(&mut self, title: String, cx: &mut Context<Self>) {
        self.is_generating = true;
        self.current_tool = None;
        self.current_file = None;
        self.files_edited.clear();
        self.turn_index = 0;
        self.thread_title = Some(title);
        self.waiting_for_approval = false;
        cx.notify();
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.is_generating = false;
        self.current_tool = None;
        self.current_file = None;
        self.files_edited.clear();
        self.turn_index = 0;
        self.thread_title = None;
        self.waiting_for_approval = false;
        cx.notify();
    }
}

/// Initialize the activity bus global. Called from `prism_hq::init`.
pub fn init(cx: &mut App) -> Entity<AgentActivityBusInner> {
    let inner = cx.new(|_cx| AgentActivityBusInner {
        is_generating: false,
        current_tool: None,
        current_file: None,
        files_edited: Vec::new(),
        turn_index: 0,
        thread_title: None,
        waiting_for_approval: false,
    });
    cx.set_global(AgentActivityBus(inner.clone()));
    inner
}

/// Get the activity bus inner entity, if initialized.
pub fn global_inner(cx: &App) -> Option<Entity<AgentActivityBusInner>> {
    cx.try_global::<AgentActivityBus>()
        .map(|bus| bus.0.clone())
}
