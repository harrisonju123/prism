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
