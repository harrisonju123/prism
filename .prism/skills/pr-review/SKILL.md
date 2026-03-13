---
name: pr-review
description: 7-step canonical PR review — diff scope, dependency tracing, DB/config checks, artifact consistency, targeted tests, and structured findings
user-invocable: true
allowed-tools: read_file, list_dir, glob_files, grep_files, bash, web_fetch, report_finding, report_blocker, recall, ask_human
---

You are running the canonical PrisM PR review workflow. Follow all 7 steps in order.
Do not skip steps. Use `report_finding` for each issue at the appropriate confidence level.

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

Report `concern`+ for:
- A field not initialized in all constructors
- A service added to AppState but not wired in the builder
- A trait method missing from one or more implementors
- A caller that was not updated to pass a new required argument

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

## Step 5 — Check generated artifact consistency

For each artifact type affected by the diff:

**Cargo.toml:**
```bash
cargo check -p <affected-crate> --features full 2>&1 | tail -20
```
Report `blocker` if compilation fails.

**OpenAPI / schema files:** Verify handler signatures match any generated schema.

**Docker Compose:** Check that new services have health checks, that volumes are named,
and that new env vars are passed through.

**Makefile:** Verify any new targets actually work (check the command, not execute it).

If no generated artifacts are affected, note "no generated artifacts affected" and continue.

---

## Step 6 — Run targeted tests

Run tests only for affected crates — do NOT run the full workspace build:

```bash
cargo test -p <affected-crate> --features full 2>&1
```

For multiple affected crates, run them in sequence.

**Analyze results:**
- Are failures pre-existing or introduced by this PR?
  - Check by running against merge base: `git stash; cargo test -p <crate>; git stash pop`
  - Or compare failure message against known pre-existing failures documented in CLAUDE.md.
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

For `blocker` and `likely_blocker`: you MUST complete the Claim Validation Protocol
(Step 3a/b/c for the specific issue) before calling `report_finding`.

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
