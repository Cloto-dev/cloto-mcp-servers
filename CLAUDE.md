# cloto-mcp-servers Development Rules

## Mandatory Reads

- **`docs/MGP_SPEC.md`** -- MGP protocol (strict MCP superset). Read before changing protocol handling.

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

## Bug Verification

- Source of truth: `qa/issue-registry.json`
- Discovery: add entry -> `bash scripts/verify-issues.sh` -> must return `[VERIFIED]`
- Fix: update `expected`->`"absent"`, `status`->`"fixed"` -> re-verify -> must return `[FIXED]`
- `scripts/verify-issues.sh` is **read-only infrastructure** -- never modify without user approval

## Adding a New MCP Server

1. Create `servers/<name>/server.py` (use `ToolRegistry` from `common/mcp_utils.py`)
2. Add `servers/<name>/pyproject.toml`
3. Add tests to `servers/tests/`
4. Register in `registry.json`
5. Add server entry to ClotoCore's `mcp.toml`

## Integration with ClotoCore

Consumed via `mcp.toml`'s `[paths].servers` with `${servers}/terminal/server.py` variable expansion.

## Git Rules

- Commit messages in English
- Git author: `ClotoCore Project <ClotoCore@proton.me>`
- Do NOT push without explicit user permission

## Prohibited

- Do NOT remove tests without updating `qa/test-baseline.json`
- Do NOT modify `scripts/verify-issues.sh` without user approval
