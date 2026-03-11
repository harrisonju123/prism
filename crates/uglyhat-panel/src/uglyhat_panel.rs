mod service;
pub use service::UglyhatService;

use gpui::App;
use workspace::Workspace;

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, cx| {
        if cx.try_global::<UglyhatService>().is_none() {
            let root = workspace
                .project()
                .read(cx)
                .visible_worktrees(cx)
                .next()
                .map(|wt| wt.read(cx).abs_path().to_path_buf());
            if let Some(root) = root {
                if let Err(e) = UglyhatService::init(&root, cx) {
                    log::warn!("uglyhat service init failed: {e}");
                }
            }
        }
    })
    .detach();
}
