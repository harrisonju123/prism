# PrisM Zed Patch Ledger

Upstream baseline: `5c481c6` (harrisonju123/zed `main` as of 2026-03-07)

Patches are stored as `git format-patch` output covering only `zed-upstream/` paths.
Apply order matters — patches must be applied in numeric order.

---

## Patch Index

| File | Patch | Change Summary | Conflict Risk |
|---|---|---|---|
| `crates/repl/src/kernels/remote_kernels.rs` | 0001 | Add `protocol_mode: Default::default()` to fix struct literal compile error | Medium — upstream may fix this in their own way |
| `crates/language_models/src/language_models.rs` | 0002 | Import + register `PrismLanguageModelProvider` | Low-Medium — provider registration list may grow |
| `crates/language_models/src/provider.rs` | 0002 | Add `pub mod prism;` | Low |
| `crates/language_models/src/provider/prism.rs` | 0002, 0004 | **NEW FILE** — full PrisM provider implementation (model discovery, streaming, settings) | Zero — no upstream counterpart; safe |
| `crates/language_models/src/settings.rs` | 0002 | Add `prism: PrismSettings` field | Low |
| `crates/settings_content/src/language_model.rs` | 0002 | Add `PrismSettingsContent`, `PrismAvailableModel` structs | Low |
| `assets/settings/default.json` | 0002, 0003, 0006 | Prism config block, default api_url, telemetry off, account UI hidden | Medium — this file is actively changed upstream |
| `crates/language_models/src/provider/open_ai.rs` | 0005 | Add `"length"` → `StopReason::MaxTokens` match arm | Medium — active file, upstream may add same mapping |
| `crates/settings_content/src/settings_content.rs` | 0006 | `TelemetrySettingsContent::default()` → both fields false | Low |
| `crates/telemetry/src/telemetry.rs` | 0006 | `send_event()` → no-op stub | Low |
| `assets/settings/default.json` | 0012 | Change `default_model.provider` from `zed.dev` to `prism` | Medium — actively changed upstream |
| `crates/language_model/src/registry.rs` | 0012 | Prioritize `prism` instead of `zed.dev` in `providers()` ordering | Low |
| `crates/settings_content/src/agent.rs` | 0012 | Replace `zed.dev` with `prism` in provider enum list | Low |
| `crates/auto_update/src/auto_update.rs` | 0013 | Disable auto-update polling unconditionally | Low |
| `crates/client/src/client.rs` | 0014 | Make `SignIn` action a no-op | Low |
| `assets/settings/default.json` | 0014 | Set `server_url` to empty string | Medium |
| `crates/feedback/src/feedback.rs` | 0015 | Remove Zed feedback URLs, make actions no-ops | Low |
| `crates/zed/src/zed/app_menus.rs` | 0015 | Remove Zed-specific Help menu items | Low |
| `crates/edit_prediction_ui/src/edit_prediction_button.rs` | 0015 | Remove `billing-support@zed.dev` reference | Low |
| `crates/ai_onboarding/src/young_account_banner.rs` | 0015 | Remove `billing-support@zed.dev` reference | Low |
| `crates/cloud_llm_client/src/cloud_llm_client.rs` | 0016 | Mark Zed-specific headers as legacy | Low |
| `crates/http_client/src/http_client.rs` | 0016 | Mark `build_zed_*_url()` functions as legacy | Low |
| `assets/settings/default.json` | 0017 | `icon_theme` → `PrisM (Default)`, `buffer_font_family` → `.PrismMono`, `ui_font_family` → `.PrismSans` | Medium |
| `crates/gpui/src/text_system.rs` | 0017 | Add `.PrismMono`/`.PrismSans` to fallback stack and font alias match arms (keep `.ZedMono`/`.ZedSans` as compat) | Low |
| `crates/editor/src/editor.rs` | 0017 | `MINIMAP_FONT_FAMILY` → `.PrismMono` | Low |
| `crates/theme/src/fallback_themes.rs` | 0017 | Rename to `prism_default_themes()`, id → `prism-default`, name → `PrisM Default` | Low |
| `crates/theme/src/registry.rs` | 0017 | Call `prism_default_themes()` | Low |
| `crates/settings/src/settings_store.rs` | 0017 | Test fixtures: `Zed Mono` → `PrisM Mono` | Low |
| `crates/editor/src/test.rs` | 0017 | `.ZedMono` → `.PrismMono` in test font | Low |
| `crates/gpui/src/text_system/line_wrapper.rs` | 0017 | `.ZedMono` → `.PrismMono` in test | Low |
| `crates/storybook/src/storybook.rs` | 0017 | `.ZedMono` → `.PrismMono` | Low |
| `crates/markdown/examples/markdown.rs` | 0017 | `.ZedSans`/`.ZedMono` → `.PrismSans`/`.PrismMono` | Low |
| `crates/markdown/examples/markdown_as_child.rs` | 0017 | `Zed Mono` → `PrisM Mono` | Low |
| `crates/language_model/src/language_model.rs` | 0018 | `ZED_CLOUD_PROVIDER_ID` value → `"prism"` | Low |
| `crates/settings_content/src/language_model.rs` | 0018 | Remove `zed_dot_dev` field | Low |
| `crates/agent_ui/src/agent_panel.rs` | 0018 | Provider check `"zed.dev"` → `"prism"` | Low |
| `crates/web_search_providers/src/cloud.rs` | 0018 | `ZED_WEB_SEARCH_PROVIDER_ID` → `"prism"` | Low |
| `assets/settings/default.json` | 0018 | Remove `"zed.dev": {}` block | Low |
| `crates/onboarding/src/onboarding.rs` et al. (~13 source files) | 0019 | `zed.dev/docs` → `prism.dev/docs` | Low |
| `assets/settings/initial_*.json`, `assets/keymaps/*.json` | 0019 | `zed.dev/docs` → `prism.dev/docs` | Low |
| `crates/auto_update_helper/src/updater.rs` | 0020 | `Zed.exe` → `PrisM.exe` | Low |
| `crates/auto_update/src/auto_update.rs` | 0020 | `Zed.exe` → `PrisM.exe` | Low |
| `crates/explorer_command_injector/src/explorer_command_injector.rs` | 0020 | `Zed.exe` → `PrisM.exe` | Low |
| `crates/cli/src/main.rs` | 0020, 0022 | Windows: `Zed.exe` → `PrisM.exe`; Linux: add `./prism` to possible_locations | Low |
| `crates/zed/resources/windows/zed.iss` | 0020 | `Zed.exe` → `PrisM.exe` | Low |
| `script/bundle-windows.ps1` | 0020 | `Zed.exe` → `PrisM.exe`, `zed.exe` → `prism.exe` | Low |
| `crates/repl/src/kernels/remote_kernels.rs` | 0021 | User-Agent `"Zed/{}"` → `"PrisM/{}"` | Low |
| `crates/copilot_chat/src/copilot_chat.rs` | 0021 | User-Agent `"Zed/{}"` → `"PrisM/{}"` | Low |
| `crates/zed/Cargo.toml` | 0022 | `description` → PrisM tagline, `default-run` → `prism`, `[[bin]] name` → `prism` | Low |
| `crates/ai_onboarding/src/ai_onboarding.rs` | 0023 | "Welcome to Zed AI/Pro/Student" → "Prism AI/Pro/Student" (8 strings) | Low |
| `crates/ai_onboarding/src/ai_upsell_card.rs` | 0023 | "Try/You're in the Zed …" → "Prism …" (5 strings) | Low |
| `crates/agent_ui/src/ui/end_trial_upsell.rs` | 0023 | "Upgrade to Zed Pro" / "Zed Pro Trial has expired" → Prism | Low |
| `crates/agent_ui/src/connection_view/thread_view.rs` | 0023 | "Upgrade to Zed Pro for more prompts" → Prism (2 occurrences) | Low |
| `crates/agent_ui/src/connection_view.rs` | 0023 | "Upgrade {} to work with Zed" → Prism | Low |
| `crates/onboarding/src/onboarding.rs` | 0023 | "Welcome to Zed" → "Welcome to Prism" | Low |
| `crates/zed/src/zed/quick_action_bar/repl_menu.rs` | 0023 | "Setup Zed REPL for {}" → "Setup Prism REPL for {}" | Low |
| `crates/acp_thread/src/acp_thread.rs` | 0024 | Cache `available_commands` on `AcpThread`; expose `available_commands()` getter | Low — additive |
| `crates/agent_ui/src/connection_view.rs` | 0024 | Initialize `available_commands` Rc from `thread.read(cx).available_commands()` at `make_thread_view` | Low |

---

## Patches

### 0001 — Compile fix: protocol_mode
**File:** `crates/repl/src/kernels/remote_kernels.rs`
**Risk:** Medium

The `JupyterWebSocket` struct added a `protocol_mode` field that wasn't initialized in our struct literal. Added `protocol_mode: Default::default()`.

**Watch for:** If upstream adds/removes fields from `JupyterWebSocket`, this patch may fail or become unnecessary.

---

### 0002 — Add PrisM as a native Zed language model provider
**Files:** 6 files (provider.rs, language_models.rs, settings.rs, language_model.rs, default.json, prism.rs NEW)
**Risk:** Low-Medium (provider registration), Zero (prism.rs)

Adds the PrisM provider to Zed's language model registry. The new file `provider/prism.rs` has no upstream counterpart so it will never conflict. Registration in `language_models.rs` and settings structs may conflict if upstream adds more providers.

**Watch for:** Changes to how providers are registered in `language_models.rs`; changes to `LanguageModelProviderSettings` struct; new fields in `default.json`'s `language_models` section.

---

### 0003 — Set PrisM default api_url
**File:** `assets/settings/default.json`
**Risk:** Medium

Sets `"api_url": "http://localhost:9100/v1"` in the prism settings block.

**Watch for:** Any upstream changes to the JSON structure around our prism block. The file is frequently edited by Zed upstream. Use a 3-way merge carefully.

---

### 0004 — Auto-discover PrisM models from /v1/models
**File:** `crates/language_models/src/provider/prism.rs`
**Risk:** Zero (our file)

Adds model auto-discovery via HTTP GET to the PrisM `/v1/models` endpoint. No upstream conflict possible.

---

### 0005 — Map Gemini FUNCTION_CALL + length finish reason in Zed
**File:** `crates/language_models/src/provider/open_ai.rs`
**Risk:** Medium

Adds `"length" => StopReason::MaxTokens` arm to a match expression. Upstream may add this same mapping. If both patches add the same arm, the duplicate will fail compilation.

**Watch for:** Upstream changes to the `finish_reason` match in `open_ai.rs`. If upstream adds `"length"` themselves, drop this patch.

---

### 0006 — Strip Zed telemetry and account UI for PrisM fork
**Files:** `assets/settings/default.json`, `crates/settings_content/src/settings_content.rs`, `crates/telemetry/src/telemetry.rs`
**Risk:** Low-Medium (default.json), Low (others)

Disables telemetry by default and stubs out `send_event()`. The `settings_content.rs` and `telemetry.rs` changes are minimal and unlikely to conflict. The `default.json` hunk may conflict with other upstream edits to that file.

**Watch for:** Upstream re-enabling or restructuring telemetry settings; changes around the `telemetry` JSON block.

---

### 0012 — Make PrisM the default LLM provider, hide zed.dev
**Files:** `assets/settings/default.json`, `crates/language_model/src/registry.rs`, `crates/settings_content/src/agent.rs`
**Risk:** Medium (default.json), Low (others)

Changes `default_model.provider` from `zed.dev` to `prism`, reorders provider list to show PrisM first, and replaces `zed.dev` with `prism` in the settings schema enum.

**Watch for:** Upstream changes to default model settings or provider ordering logic.

---

### 0013 — Disable auto-update polling
**File:** `crates/auto_update/src/auto_update.rs`
**Risk:** Low

Unconditionally disables auto-update polling so no background HTTP calls are made to Zed release servers.

**Watch for:** Upstream restructuring of update init logic.

---

### 0014 — Disable Zed collaboration and sign-in
**Files:** `crates/client/src/client.rs`, `assets/settings/default.json`
**Risk:** Low-Medium

Makes `SignIn` action a no-op and sets `server_url` to empty string. No WebSocket connections to collab.zed.dev.

**Watch for:** Upstream changes to sign-in flow or `ClientSettings`.

---

### 0015 — Remove Zed feedback actions and documentation URLs
**Files:** `crates/feedback/src/feedback.rs`, `crates/zed/src/zed/app_menus.rs`, `crates/edit_prediction_ui/src/edit_prediction_button.rs`, `crates/ai_onboarding/src/young_account_banner.rs`
**Risk:** Low

Removes Zed GitHub/email feedback actions (makes them no-ops), removes Help menu items pointing to zed.dev, and strips `billing-support@zed.dev` email references.

**Watch for:** New menu items added upstream; changes to feedback action signatures.

---

### 0016 — Clean up cloud LLM client headers
**Files:** `crates/cloud_llm_client/src/cloud_llm_client.rs`, `crates/http_client/src/http_client.rs`
**Risk:** Low

Marks Zed-specific `x-zed-*` header constants and `build_zed_*_url()` functions as legacy. These become unreachable when `server_url` is not `zed.dev`.

**Watch for:** New header constants added upstream; changes to URL builder function signatures.

---

### 0017 — Font, theme, and icon branding
**Files:** `assets/settings/default.json`, `crates/gpui/src/text_system.rs`, `crates/editor/src/editor.rs`, `crates/theme/src/fallback_themes.rs`, `crates/theme/src/registry.rs`, `crates/settings/src/settings_store.rs`, `crates/editor/src/test.rs`, `crates/gpui/src/text_system/line_wrapper.rs`, `crates/storybook/src/storybook.rs`, `crates/markdown/examples/markdown.rs`, `crates/markdown/examples/markdown_as_child.rs`
**Risk:** Low-Medium (default.json), Low (others)

Adds `.PrismMono`/`.PrismSans` virtual font aliases that resolve to the same underlying fonts as `.ZedMono`/`.ZedSans` (Lilex and IBM Plex Sans). Keeps `.ZedMono`/`.ZedSans` as backward-compat fallbacks in the match arms. Sets PrisM fonts/icon theme as defaults. Renames fallback theme function and IDs to `prism-*`.

**Watch for:** Upstream changes to `font_name_with_fallbacks` match arms; changes around `fallback_font_stack` initialization.

---

### 0018 — Remove zed.dev provider
**Files:** `crates/language_model/src/language_model.rs`, `crates/settings_content/src/language_model.rs`, `crates/agent_ui/src/agent_panel.rs`, `crates/web_search_providers/src/cloud.rs`, `assets/settings/default.json`
**Risk:** Low

Changes `ZED_CLOUD_PROVIDER_ID` to `"prism"`, removes the `zed_dot_dev` settings field, updates the provider string check in agent panel, and strips the `"zed.dev": {}` block from default settings. PrisM is now the only cloud provider.

**Watch for:** Upstream adding new provider IDs; changes to `LanguageModelSettingsContent` struct fields.

---

### 0019 — Documentation URLs
**Files:** ~13 source files, 9 asset JSON/keymap files
**Risk:** Low

Mechanical replacement of `zed.dev/docs` → `prism.dev/docs` in all user-visible runtime files. Does not touch `docs/`, `legal/`, or `script/` directories.

**Watch for:** Upstream adding new `zed.dev/docs` references in runtime code.

---

### 0020 — Windows binary name
**Files:** `crates/auto_update_helper/src/updater.rs`, `crates/auto_update/src/auto_update.rs`, `crates/explorer_command_injector/src/explorer_command_injector.rs`, `crates/cli/src/main.rs`, `crates/zed/resources/windows/zed.iss`, `script/bundle-windows.ps1`
**Risk:** Low

Renames `Zed.exe` → `PrisM.exe` (and `zed.exe` → `prism.exe`) across Windows-specific paths. Also adds `./prism` to the Linux `possible_locations` binary search list in `cli/src/main.rs`.

**Watch for:** Upstream changes to Windows updater logic or binary path discovery.

---

### 0021 — User-Agent strings
**Files:** `crates/repl/src/kernels/remote_kernels.rs`, `crates/copilot_chat/src/copilot_chat.rs`
**Risk:** Low

Changes HTTP User-Agent header values from `"Zed/{version}"` to `"PrisM/{version}"`.

**Watch for:** New user-agent strings added upstream.

---

### 0022 — Binary name and crate description
**Files:** `crates/zed/Cargo.toml`, `crates/cli/src/main.rs`
**Risk:** Low

Updates `description` to PrisM tagline, `default-run` to `prism`, and `[[bin]] name` to `prism` so `cargo build -p zed` produces a `prism` binary. The `[package] name` stays `"zed"` to avoid cascading import changes. Adds `./prism` alongside `./zed` in Linux binary discovery.

**Watch for:** Upstream adding new `[[bin]]` sections; changes to how the CLI discovers the main binary.

---

### 0023 — Rebrand remaining user-visible "Zed" strings to "Prism"
**Files:** `crates/ai_onboarding/src/ai_onboarding.rs`, `crates/ai_onboarding/src/ai_upsell_card.rs`, `crates/agent_ui/src/ui/end_trial_upsell.rs`, `crates/agent_ui/src/connection_view/thread_view.rs`, `crates/agent_ui/src/connection_view.rs`, `crates/onboarding/src/onboarding.rs`, `crates/zed/src/zed/quick_action_bar/repl_menu.rs`
**Risk:** Low

Mechanical replacement of user-visible "Zed" brand strings (welcome screens, plan names, upsell CTAs) to "Prism". Internal identifiers (`Plan::ZedPro`, `VectorName::ZedLogo`, `zed_urls::*`) left unchanged.

**Watch for:** Upstream adding new onboarding/upsell strings referencing "Zed".

---

## Audit Log

### 2026-03-07 — Audit vs upstream HEAD (already up to date)

`./scripts/sync-zed-upstream.sh --dry-run` confirmed `harrisonju123/zed main` has not
advanced past baseline `5c481c686a7eb542047ed231c1b95c15bdc95b8b`. No upstream delta to
apply. All 10 patched files are unchanged upstream; risk ratings remain as documented above.

---

## After Each Sync

1. Run `cargo check -p zed` to verify all patches applied cleanly
2. If a patch becomes unnecessary (upstream fixed the issue), remove it from this ledger and delete the `.patch` file
3. If a patch fails to apply, resolve manually and regenerate with `git format-patch`
4. Update `BASELINE` to the new SHA after a clean sync
