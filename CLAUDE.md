# cloto-mcp-servers Development Rules

## Mandatory Reads

Read these before making changes. Do not summarize — read the actual files (feedback_doc_firsthand_reading).

- **`docs/MGP_SPEC.md`** — MGP protocol (strict MCP superset). Read before changing protocol handling.

## Commands

- Test: `cd servers && python -m pytest tests/ -v`
- Lint: `ruff check servers/`
- Format: `ruff format servers/`
- Format check: `ruff format --check servers/`
- Auto-fix: `ruff check --fix servers/`
- Bug verify: `bash scripts/verify-issues.sh`
- Test ratchet: `bash scripts/check-test-count.sh`
- Sentinel: `bash scripts/sentinel.sh`

## Rust Server Commands (servers/avatar/)

- Build: `cd servers/avatar && cargo build`
- Lint: `cd servers/avatar && cargo clippy -- -D warnings`
- Format check: `cd servers/avatar && cargo fmt -- --check`
- Format: `cd servers/avatar && cargo fmt`

## Rust Server Commands (servers/discord/)

- Build: `cd servers/discord && cargo build`
- Lint: `cd servers/discord && cargo clippy -- -D warnings`
- Format check: `cd servers/discord && cargo fmt -- --check`
- Format: `cd servers/discord && cargo fmt`

## Bug Verification (Anti-Hallucination)

> Inherits: `../CLAUDE.md` — "Mandatory: Issue Registry Verification" section.
> MUST run `bash scripts/verify-issues.sh` when adding / fixing / claiming a fix for
> entries in `qa/issue-registry.json`. PostToolUse hook auto-runs on edits;
> `.githooks/pre-commit` blocks commits with `[STALE]` / `[UNFIXED]`.

- Source of truth: `qa/issue-registry.json`
- Scope: bugs where code-level evidence is needed (e.g., AI-discovered bugs that could be false positives). Not every fix needs an entry.
- **Enable pre-commit blocker (once per clone)**: `bash scripts/install-hooks.sh` — sets `core.hooksPath=.githooks`. Baseline: clean as of 2026-04-24 (all 7 entries `[VERIFIED]`).

## Adding a New MCP Server

1. Create `servers/<name>/server.py` (use `ToolRegistry` from `common/mcp_utils.py`)
2. Add `servers/<name>/pyproject.toml`
3. Add tests to `servers/tests/`
4. Register in `registry.json`
5. Add server entry to ClotoCore's `mcp.toml`

## Integration with ClotoCore

Consumed via `mcp.toml`'s `[paths].servers` with `${servers}/terminal/server.py` variable expansion.

## Git Rules

> Inherits: `../CLAUDE.md` — shared Git Rules section (author, English commits, push = explicit instruction only, never bundle push into commit).

## Prohibited

- Do NOT remove tests without updating `qa/test-baseline.json`
