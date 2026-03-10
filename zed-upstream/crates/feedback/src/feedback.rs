use gpui::{App, ClipboardItem, PromptLevel, actions};
use system_specs::{CopySystemSpecsIntoClipboard, SystemSpecs};
use util::ResultExt;
use workspace::Workspace;
use zed_actions::feedback::{EmailZed, FileBugReport, RequestFeature};

actions!(
    zed,
    [
        /// No-op: Zed repository link removed in PrisM.
        OpenZedRepo,
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace
            .register_action(|_, _: &CopySystemSpecsIntoClipboard, window, cx| {
                let specs = SystemSpecs::new(window, cx);

                cx.spawn_in(window, async move |_, cx| {
                    let specs = specs.await.to_string();

                    cx.update(|_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(specs.clone()))
                    })
                    .log_err();

                    cx.prompt(
                        PromptLevel::Info,
                        "Copied into clipboard",
                        Some(&specs),
                        &["OK"],
                    )
                    .await
                })
                .detach();
            })
            // PrisM: feedback actions are no-ops — Zed.dev feedback channels removed
            .register_action(|_, _: &RequestFeature, _, _cx| {})
            .register_action(move |_, _: &FileBugReport, _window, _cx| {})
            .register_action(move |_, _: &EmailZed, _window, _cx| {})
            .register_action(move |_, _: &OpenZedRepo, _, _cx| {});
    })
    .detach();
}
