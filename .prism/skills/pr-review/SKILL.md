---
name: pr-review
description: 8-step canonical PR review — rules loading, diff scope, dependency tracing, DB/config checks, artifact consistency, targeted tests, and structured findings
user-invocable: true
allowed-tools: read_file, list_dir, glob_files, grep_files, bash, web_fetch, report_finding, report_blocker, recall, ask_human
---

You are running the canonical PrisM PR review workflow. Follow all 8 steps in order.
Do not skip steps. Use `report_finding` for each issue at the appropriate confidence level.

---

## Step 0 — Load review rules and repo conventions

**a. Load repo-specific rules:**

Try to read `.prism/review-rules.toml` with `read_file .prism/review-rules.toml`.

- If the file exists and parses successfully: store all sections. Note which sections are
  present (`[general]`, `[rust]`, `[go]`, `[python]`, `[typescript]`).
- If the file exists but contains invalid TOML: report the parse error (with line context if
  visible), halt Step 0, and ask the user to fix the syntax before proceeding.
- If the file does NOT exist: note its absence. After completing Step 1, suggest the user
  create one based on `.prism/skills/pr-review/default-review-rules.toml`. Continue the
  review with an empty rules set (no required_checks, no forbidden_patterns, no conventions).

**b. Load CLAUDE.md conventions:**

Read `CLAUDE.md` with `read_file CLAUDE.md`. Extract:
- The **Conventions** section — used in Step 3e to check convention compliance in new code
- The **Known Issues** section — used in Step 6 to determine if test failures are pre-existing

Store both for reference throughout the review.

**c. Define active language sections:**

You do not yet know which files changed — defer language-section activation until after
Step 1 establishes the diff scope.

When Step 1 completes, set:
```
active_sections = {[general]} ∪ {[<language>] for each language found in changed files}
  .rs files  → add [rust]
  .go files  → add [go]
  .py files  → add [python]
  .ts / .js  → add [typescript]
```

In all subsequent steps, "from the active sections" means: collect the relevant field from
every section in `active_sections`, then merge/concatenate the lists.

---

## Step 1 — Fetch PR branch & establish diff scope

If given a PR number or GitHub URL:
```bash
gh pr view <number> --json number,title,baseRefName,headRefName,body,commits
gh pr diff <number> --name-only
```

If given a branch name directly:
```bash
git fetch origin <branch>
MERGE_BASE=$(git merge-base main origin/<branch>)
git diff --stat $MERGE_BASE origin/<branch>
git log --oneline $MERGE_BASE..origin/<branch>
```

Record:
- **Merge base** commit SHA
- **Changed files** list (full paths)
- **Commit messages** (to understand intent)

**After recording changed files:** activate language sections per Step 0c.

For Rust PRs, derive `affected_crates` (a list) from changed file paths:
- `crates/prism-cli/` → `prism-cli`
- `crates/prism/` → `prism`
- Multiple affected directories → multiple entries in the list

Substitute `{changed_files}` with the space-separated list of changed file paths.
`{affected_crate}` (singular) in rule templates is a placeholder for one crate at a time —
when running a command for multiple crates, run the command once per crate (see Step 5a).

If you cannot fetch the branch, ask the user for the correct remote/branch name before proceeding.

---

## Step 2 — Read changed files with rooted paths

For each file in the changed files list:

1. Read the full file with `read_file <full-path-from-repo-root>`.
2. For large files (>500 lines): use `bash git diff $MERGE_BASE origin/<branch> -- <path>` to
   see only the changed hunks, then use `read_file` with `offset`/`limit` to read surrounding
   context for each hunk.
3. Build a mental model:
   - New types, structs, enums, traits introduced
   - Modified functions and their signatures
   - Deleted code (check for orphaned references)
   - New dependencies in Cargo.toml

Do NOT proceed to Step 3 until you have read every changed file.

---

## Step 3 — Trace all new dependencies & DI wiring

For **every** new struct field, function parameter, service, or trait implementation in the diff:

**a. Find where it is constructed:**
```
grep_files "::new\(" <repo-root>
grep_files "<TypeName>" <repo-root> --file-glob "*.rs"
```

**b. Find where it is registered** (AppState, router, builder pattern):
```
grep_files "with_<field_name>\|<field_name>:" <repo-root> --file-glob "*.rs"
grep_files "AppState\|AppStateBuilder" <repo-root> --file-glob "*.rs"
```

**c. Find all call sites** — are all callers updated?
```
grep_files "<function_name>\|<method_name>" <repo-root> --file-glob "*.rs"
```

**d. Check trait implementations** — does every implementor provide the new method?
```
grep_files "impl <TraitName> for" <repo-root> --file-glob "*.rs"
```

**e. Check forbidden patterns and conventions (from Step 0 rules):**

Batch all `forbidden_patterns` from the active sections into a single grep call:
```bash
git diff $MERGE_BASE -- <changed-files> | grep '^+' | grep -v '^+++' | \
  grep -E "<pattern1>|<pattern2>|<patternN>"
```
Using `git diff` output (lines prefixed with `+`) ensures only new or modified lines are
checked — not pre-existing code that was already there before this PR. Report `concern` for
each pattern match.

For each entry in `conventions` from the active sections:
Check whether new code introduced in the diff follows the convention. To distinguish new
from pre-existing, refer to the `+`-prefixed lines in `git diff $MERGE_BASE -- <file>`.
Only report violations that appear on `+` lines (newly added or modified lines). Report:
- Naming-only violations → `nit`
- Structural violations (missing Default impl, wrong module naming) → `concern`

Report `concern`+ for:
- A field not initialized in all constructors
- A service added to AppState but not wired in the builder
- A trait method missing from one or more implementors
- A caller that was not updated to pass a new required argument
- A forbidden pattern found in new code

---

## Step 4 — Check DB / config / env initialization paths

**SQL Migrations:**
- Does the migration file exist in `migrations/postgres/` or the appropriate directory?
- Are column types correct and nullable only when intentional?
- Are indexes added for columns used in WHERE / JOIN clauses?
- Is the migration registered in `main.rs` (search for the migration filename)?
- For ClickHouse: is the schema added to `observability/schema.rs`'s `MIGRATIONS` array?

```bash
grep -n "migration\|sql" crates/prism/src/main.rs | head -40
```

**Config fields:**
- Does the new config struct have a `Default` impl with a sensible value?
- Is there an env var override path (Figment or manual env read)?
- Is the field documented (at minimum a doc comment)?

**Environment variables:**
- Search for all references to the new env var:
  ```
  grep_files "PRISM_<VAR>\|env::var" <repo-root> --file-glob "*.rs"
  ```
- Does it have a documented default? Does it fail gracefully if absent?

Report `likely_blocker` for:
- A migration unregistered in `main.rs`
- A required config field with no `Default` and no env fallback
- A required env var with no documented default that would panic if unset

---

## Step 5 — Check generated artifact consistency and run required checks

**Initialize tracking:**
```
tested_crates = []   # crates for which cargo test has already been run in this step
```

**5a. Run required checks from review rules:**

Collect all `required_checks` from the active sections. For each command:

1. Substitute `{changed_files}` → space-separated changed file paths.
2. Substitute `{affected_crate}` → run the command **once per crate** in `affected_crates`.
   (Commands like `cargo clippy -p <crate>` accept only one `-p` argument at a time.)

```bash
# Example for two affected crates: prism and prism-cli
cargo clippy -p prism --features full -- -W clippy::all 2>&1
cargo clippy -p prism-cli --features full -- -W clippy::all 2>&1
```

After running, append to `tested_crates` any crate for which a `cargo test` command was run.

**Deduplication:**
- If `required_checks` includes `cargo clippy` for a crate, **skip** the standalone
  `cargo check` for that crate in Step 5b.
- If `required_checks` includes `cargo test` for a crate, it is added to `tested_crates`
  and will be skipped in Step 6.

Interpret results:
- Non-zero exit on a compilation check → `blocker`
- Non-zero exit on a lint/fmt check → `concern`
- Non-zero exit on a test run → treat the same as Step 6 (pre-existing vs new failure analysis)

**5b. Cargo.toml (if not already covered by required_checks):**
```bash
cargo check -p <affected-crate> --features full 2>&1 | tail -20
```
Report `blocker` if compilation fails.

**5c. OpenAPI / schema files:** Verify handler signatures match any generated schema.

**5d. Docker Compose:** Check that new services have health checks, that volumes are named,
and that new env vars are passed through.

**5e. Makefile:** Verify any new targets actually work (check the command, not execute it).

If no generated artifacts are affected, note "no generated artifacts affected" and continue.

---

## Step 6 — Run targeted tests

**Skip any crate in `tested_crates`** (already tested in Step 5a).

For remaining affected crates, run tests only for those crates — do NOT run the full workspace build:

```bash
cargo test -p <affected-crate> --features full 2>&1
```

For multiple affected crates, run them in sequence.

**Analyze results:**
- Are failures pre-existing or introduced by this PR?
  - Check by running against merge base: `git stash; cargo test -p <crate>; git stash pop`
  - Or compare failure message against the **Known Issues** section extracted from CLAUDE.md in Step 0b.
- Do new tests exercise real code paths, or do they mock away the interesting parts?
  - A test that only exercises mocks and would fail on real infrastructure is a `concern`.

**Do NOT:**
- Run `cargo build --release`
- Run `cargo test` (full workspace — too slow)
- Run tests more than twice for the same crate

---

## Step 7 — Produce findings summary

**Call `report_finding` for each issue:**

```
report_finding
  confidence=<blocker|likely_blocker|concern|nit>
  title="<short title>"
  description="<what it is, where in the code, why it matters>"
  [initialization_trace="<file:line chain>"]   # required for blocker/likely_blocker
  [reachability="<is this reachable in prod?>"] # required for blocker/likely_blocker
  [alternative_handlers="<is there fallback?>"] # required for blocker
```

For `blocker` and `likely_blocker`: you MUST complete the **Claim Validation Protocol**
before calling `report_finding`. The protocol (defined in the pr-reviewer persona) is:
1. **Trace initialization** — grep_files + read_file to follow assignment chains and
   constructors. Record file paths and line numbers.
2. **Check reachability** — look for feature flags, env gates, conditional compilation.
   A condition only in test mocks is NOT a production blocker.
3. **Check alternative handlers** — search for catch blocks, fallback branches, retry
   logic, default values, or upstream validation that already handles the condition.

If any step shows the issue is not a real blocker, downgrade to `concern` or do not report.

**Cap:**
- All blockers and likely_blockers — report every one
- Up to 5 concerns — pick the highest-impact ones
- Omit nits beyond 3, and note "N additional nits omitted"

**Final text summary (output as plain text after all report_finding calls):**

```
## PR Review Summary

**Branch / PR:** <name or number>
**Commits reviewed:** <N> (<merge-base>..<head>)
**Files reviewed:** <N> (<list key files>)

**Test results:**
- <crate>: <N passed / M failed>

**Rules compliance:**
- Checks passed: <list of required_checks commands that exited 0, or "none run">
- Checks failed: <list of required_checks commands that exited non-zero, or "none">
- Convention violations: <N> (or "none")
- Forbidden patterns found: <N> (or "none")

**Findings:**
- Blockers: <N>
- Likely blockers: <N>
- Concerns: <N>
- Nits: <N>

**Merge assessment:** <APPROVE | APPROVE WITH COMMENTS | REQUEST CHANGES>
<1-3 sentence rationale>
```

---

## Quick reference: confidence calibration

| Confidence | Example |
|---|---|
| `blocker` | New migration not registered in main.rs — will panic on startup |
| `likely_blocker` | AppState field always None because builder call missing in main.rs |
| `concern` | New config field has no env override — must redeploy to change |
| `nit` | Inconsistent naming (camelCase vs snake_case) in a doc comment |
