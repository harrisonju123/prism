use uglyhat::model::{DecisionScope, HandoffMode};

/// State for the inline "Add Memory" form shown in ThreadViewItem.
pub struct AddMemoryForm {
    pub key_input: String,
    pub value_input: String,
    pub saving: bool,
    pub active_field: MemoryField,
}

#[derive(Default, Clone, Copy, PartialEq)]
pub enum MemoryField {
    #[default]
    Key,
    Value,
}

impl Default for AddMemoryForm {
    fn default() -> Self {
        Self {
            key_input: String::new(),
            value_input: String::new(),
            saving: false,
            active_field: MemoryField::Key,
        }
    }
}

/// State for the inline "Record Decision" form shown in ThreadViewItem.
pub struct RecordDecisionForm {
    pub title_input: String,
    pub content_input: String,
    pub scope: DecisionScope,
    pub saving: bool,
    pub active_field: DecisionField,
}

#[derive(Default, Clone, Copy, PartialEq)]
pub enum DecisionField {
    #[default]
    Title,
    Content,
}

impl RecordDecisionForm {
    pub fn new() -> Self {
        Self {
            title_input: String::new(),
            content_input: String::new(),
            scope: DecisionScope::Thread,
            saving: false,
            active_field: DecisionField::Title,
        }
    }
}

/// State for the inline "Create Handoff" form shown in ThreadViewItem.
pub struct CreateHandoffForm {
    pub task_input: String,
    pub mode: HandoffMode,
    pub saving: bool,
}

impl CreateHandoffForm {
    pub fn new() -> Self {
        Self {
            task_input: String::new(),
            mode: HandoffMode::DelegateAndForget,
            saving: false,
        }
    }
}
