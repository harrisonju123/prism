mod add_dir_tool;
mod ask_human_tool;
mod escalate_decision_tool;
mod skill_tool;
mod codebase_search_tool;
mod context_handle;
mod context_overview_tool;
mod context_server_registry;
mod create_snapshot_tool;
mod copy_path_tool;
mod create_directory_tool;
mod delete_path_tool;
mod diagnostics_tool;
mod edit_file_tool;
mod fetch_tool;
mod find_path_tool;
mod forget_memory_tool;
mod grep_tool;
mod list_directory_tool;
mod list_memories_tool;
mod list_snapshots_tool;
mod lsp_tool;
mod move_path_tool;
mod notebook_edit_tool;
mod now_tool;
mod open_tool;
mod read_file_tool;
mod flag_risk_tool;
mod record_decision_tool;
mod submit_questions_tool;
mod recall_tool;
mod request_review_tool;
mod update_mission_tool;
mod update_risk_tool;
mod restore_file_from_disk_tool;
mod save_file_tool;
mod save_memory_tool;
mod send_message_tool;
mod spawn_agent_tool;
mod streaming_edit_file_tool;
mod task_create_tool;
mod task_get_tool;
mod task_list_tool;
mod task_store;
mod task_update_tool;
mod terminal_tool;
mod thread_archive_tool;
mod thread_create_tool;
mod thread_list_tool;
mod postman;
mod tool_edit_parser;
mod tool_permissions;
mod web_search_tool;

use crate::AgentTool;
use language_model::{LanguageModelRequestTool, LanguageModelToolSchemaFormat};

pub use add_dir_tool::*;
pub use ask_human_tool::*;
pub use escalate_decision_tool::*;
pub use skill_tool::*;
pub use codebase_search_tool::*;
pub use context_handle::*;
pub use context_overview_tool::*;
pub use context_server_registry::*;
pub use copy_path_tool::*;
pub use create_directory_tool::*;
pub use create_snapshot_tool::*;
pub use delete_path_tool::*;
pub use diagnostics_tool::*;
pub use edit_file_tool::*;
pub use fetch_tool::*;
pub use find_path_tool::*;
pub use flag_risk_tool::*;
pub use forget_memory_tool::*;
pub use grep_tool::*;
pub use list_directory_tool::*;
pub use list_memories_tool::*;
pub use list_snapshots_tool::*;
pub use lsp_tool::*;
pub use move_path_tool::*;
pub use notebook_edit_tool::*;
pub use now_tool::*;
pub use open_tool::*;
pub use read_file_tool::*;
pub use recall_tool::*;
pub use record_decision_tool::*;
pub use request_review_tool::*;
pub use restore_file_from_disk_tool::*;
pub use save_file_tool::*;
pub use save_memory_tool::*;
pub use send_message_tool::*;
pub use spawn_agent_tool::*;
pub use submit_questions_tool::*;
pub use streaming_edit_file_tool::*;
pub use task_create_tool::*;
pub use task_get_tool::*;
pub use task_list_tool::*;
pub use task_store::*;
pub use task_update_tool::*;
pub use terminal_tool::*;
pub use thread_archive_tool::*;
pub use thread_create_tool::*;
pub use thread_list_tool::*;
pub use tool_permissions::*;
pub use update_mission_tool::*;
pub use update_risk_tool::*;
pub use web_search_tool::*;
pub use postman::*;

macro_rules! tools {
    ($($tool:ty),* $(,)?) => {
        /// Every built-in tool name, determined at compile time.
        pub const ALL_TOOL_NAMES: &[&str] = &[
            $(<$tool>::NAME,)*
        ];

        const _: () = {
            const fn str_eq(a: &str, b: &str) -> bool {
                let a = a.as_bytes();
                let b = b.as_bytes();
                if a.len() != b.len() {
                    return false;
                }
                let mut i = 0;
                while i < a.len() {
                    if a[i] != b[i] {
                        return false;
                    }
                    i += 1;
                }
                true
            }

            const NAMES: &[&str] = ALL_TOOL_NAMES;
            let mut i = 0;
            while i < NAMES.len() {
                let mut j = i + 1;
                while j < NAMES.len() {
                    if str_eq(NAMES[i], NAMES[j]) {
                        panic!("Duplicate tool name in tools! macro");
                    }
                    j += 1;
                }
                i += 1;
            }
        };

        /// Returns whether the tool with the given name supports the given provider.
        pub fn tool_supports_provider(name: &str, provider: &language_model::LanguageModelProviderId) -> bool {
            $(
                if name == <$tool>::NAME {
                    return <$tool>::supports_provider(provider);
                }
            )*
            false
        }

        /// A list of all built-in tools
        pub fn built_in_tools() -> impl Iterator<Item = LanguageModelRequestTool> {
            fn language_model_tool<T: AgentTool>() -> LanguageModelRequestTool {
                LanguageModelRequestTool {
                    name: T::NAME.to_string(),
                    description: T::description().to_string(),
                    input_schema: T::input_schema(LanguageModelToolSchemaFormat::JsonSchema).to_value(),
                    use_input_streaming: T::supports_input_streaming(),
                }
            }
            [
                $(
                    language_model_tool::<$tool>(),
                )*
            ]
            .into_iter()
        }
    };
}

tools! {
    AddDirTool,
    AskHumanTool,
    EscalateDecisionTool,
    CodebaseSearchTool,
    ContextOverviewTool,
    CopyPathTool,
    CreateDirectoryTool,
    CreateSnapshotTool,
    DeletePathTool,
    DiagnosticsTool,
    EditFileTool,
    FetchTool,
    FindPathTool,
    FlagRiskTool,
    ForgetMemoryTool,
    GrepTool,
    ListDirectoryTool,
    ListMemoriesTool,
    ListSnapshotsTool,
    LspTool,
    MovePathTool,
    NotebookEditTool,
    NowTool,
    OpenTool,
    ReadFileTool,
    RecallTool,
    RecordDecisionTool,
    RequestReviewTool,
    RestoreFileFromDiskTool,
    SaveFileTool,
    SaveMemoryTool,
    SendMessageTool,
    SpawnAgentTool,
    SubmitQuestionsTool,
    TaskCreateTool,
    TaskGetTool,
    TaskListTool,
    TaskUpdateTool,
    TerminalTool,
    ThreadArchiveTool,
    ThreadCreateTool,
    ThreadListTool,
    UpdateMissionTool,
    UpdateRiskTool,
    WebSearchTool,
}
