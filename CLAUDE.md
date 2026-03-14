# cloto-mcp-servers Development Rules

## Overview

This repository contains all Python MCP/MGP servers for the ClotoCore project.
Extracted from the ClotoCore monorepo (https://github.com/Cloto-dev/ClotoCore).

## MGP (Model General Protocol) -- MANDATORY

MGP is a strict superset of MCP. Read `docs/MGP_SPEC.md` and related MGP docs
in `docs/` before making changes to any server's protocol handling.

## Project Structure

- `servers/` -- all MCP server implementations + common module + tests
- `servers/common/` -- shared utilities (ToolRegistry, validation, LLM provider, search)
- `servers/tests/` -- pytest test suite
- `docs/` -- MGP specification and design documents
- `dev-notes/` -- internal audit and evaluation notes

## Adding a New MCP Server

1. Create `servers/<name>/server.py`
2. Use `ToolRegistry` from `common/mcp_utils.py`
3. Add a `servers/<name>/pyproject.toml`
4. Add tests to `servers/tests/`
5. Add the server entry to ClotoCore's `mcp.toml`

## Integration with ClotoCore

This repo is consumed by ClotoCore via `mcp.toml`'s `[paths]` section:

```toml
[paths]
servers = "/path/to/cloto-mcp-servers/servers"
```

Server args use `${servers}/terminal/server.py` variable expansion.

## Git Rules

- Commit messages in English
- Git author: `ClotoCore Project <ClotoCore@proton.me>`
- Do NOT push without explicit user permission

## Testing

```bash
cd servers && python -m pytest tests/ -v
```
