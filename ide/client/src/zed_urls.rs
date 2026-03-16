//! Contains helper functions for constructing URLs to various PrisM-related pages.

use gpui::App;
use settings::Settings;

use crate::ClientSettings;

pub const PRISM_REPO: &str = "https://github.com/harrisonju123/PrisM";
pub const PRISM_QUICK_START: &str = "https://github.com/harrisonju123/PrisM#quick-start";
pub const PRISM_COMMITS_NIGHTLY: &str = "https://github.com/harrisonju123/PrisM/commits/nightly/";
pub const PRISM_COMMITS_MAIN: &str = "https://github.com/harrisonju123/PrisM/commits/main/";
pub const PRISM_EDIT_PREDICTION_DOCS: &str =
    "https://github.com/harrisonju123/PrisM/blob/main/docs/edit-prediction.md";

fn server_url(cx: &App) -> &str {
    &ClientSettings::get_global(cx).server_url
}

/// Returns the URL to the account page.
pub fn account_url(cx: &App) -> String {
    format!("{server_url}/account", server_url = server_url(cx))
}

/// Returns the URL to the terms of service.
pub fn terms_of_service(cx: &App) -> String {
    format!("{server_url}/terms-of-service", server_url = server_url(cx))
}

pub fn shared_agent_thread_url(session_id: &str) -> String {
    format!("prism://agent/shared/{}", session_id)
}
