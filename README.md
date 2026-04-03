# cloto-mcp-servers

MCP server collection for the [ClotoCore](https://github.com/Cloto-dev/ClotoCore) platform.

## CPersona — AI Memory Server

**CPersona** (`servers/cpersona/`) is an MCP Memory Server that gives Claude persistent memory.
See the [cpersona README](servers/cpersona/README.md) for full documentation.

- 3-layer hybrid search (vector + FTS5 full-text + keyword) with RRF merge
- Confidence scoring with dynamic time decay, recall boost, and completion factor
- Episodic memory (conversation summarization) and profile memory (user attributes)
- Zero LLM dependency — pure data server, all intelligence stays in the calling agent
- 16 tools including health check, auto-calibration, JSONL export/import, and agent merge
- Agent namespace isolation
- stdio + Streamable HTTP transport
- Single-file SQLite DB (schema v7, auto-migrating)

**License: MIT** — free to use from any MCP host.

### Quick Start (Claude Desktop / Claude Code)

```bash
git clone https://github.com/Cloto-dev/cloto-mcp-servers.git
cd cloto-mcp-servers/servers
python -m venv .venv

# Windows
.venv\Scripts\activate

# macOS / Linux
# source .venv/bin/activate

pip install -r requirements.lock
```

**Claude Desktop** — add to `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "embedding": {
      "command": "/path/to/servers/.venv/bin/python",
      "args": ["/path/to/servers/embedding/server.py"],
      "env": {
        "EMBEDDING_PROVIDER": "onnx_jina_v5_nano",
        "EMBEDDING_HTTP_PORT": "8401"
      }
    },
    "cpersona": {
      "command": "/path/to/servers/.venv/bin/python",
      "args": ["/path/to/servers/cpersona/server.py"],
      "env": {
        "CPERSONA_DB_PATH": "/home/yourname/.claude/cpersona.db",
        "CPERSONA_EMBEDDING_MODE": "http",
        "CPERSONA_EMBEDDING_URL": "http://127.0.0.1:8401/embed",
        "CPERSONA_VECTOR_SEARCH_MODE": "remote"
      }
    }
  }
}
```

> **Windows**: use `.venv/Scripts/python.exe` instead of `.venv/bin/python`, and `C:/Users/yourname/.claude/cpersona.db` for the DB path.

> **Note**: Claude Desktop on Windows has [known issues with stdio MCP transport](https://github.com/anthropics/claude-code/issues/36319) (payload size limits, silent drops). If you experience problems, consider using cpersona's Streamable HTTP transport instead. See the [Zenn Book Ch.2](https://zenn.dev/clotodev/books/claude-memory-mcp-server/viewer/ch02-quickstart) for details.

**Claude Code**:

```bash
# macOS / Linux
claude mcp add-json embedding '{
  "type": "stdio",
  "command": "/path/to/servers/.venv/bin/python",
  "args": ["/path/to/servers/embedding/server.py"],
  "env": {
    "EMBEDDING_PROVIDER": "onnx_jina_v5_nano",
    "EMBEDDING_HTTP_PORT": "8401"
  }
}' -s user

claude mcp add-json cpersona '{
  "type": "stdio",
  "command": "/path/to/servers/.venv/bin/python",
  "args": ["/path/to/servers/cpersona/server.py"],
  "env": {
    "CPERSONA_DB_PATH": "/home/yourname/.claude/cpersona.db",
    "CPERSONA_EMBEDDING_MODE": "http",
    "CPERSONA_EMBEDDING_URL": "http://127.0.0.1:8401/embed",
    "CPERSONA_VECTOR_SEARCH_MODE": "remote"
  }
}' -s user
```

Adjust paths to match your environment. For detailed setup instructions, see the [Zenn Book Ch.2](https://zenn.dev/clotodev/books/claude-memory-mcp-server/viewer/ch02-quickstart).

## All Servers

| ID | Type | Description |
|----|------|-------------|
| `memory.cpersona` | Memory | CPersona persistent memory with FTS5 search (MIT) |
| `tool.embedding` | Tool | Vector embedding generation (ONNX + API providers) |
| `tool.terminal` | Tool | Sandboxed command execution |
| `tool.agent_utils` | Tool | Deterministic utilities (time, math, UUID, units, encode/decode) |
| `tool.cron` | Tool | CRON job management |
| `tool.websearch` | Tool | Web search integration (SearXNG, Tavily, DuckDuckGo) |
| `tool.research` | Tool | Research synthesis with search and LLM delegation |
| `tool.imagegen` | Tool | Stable Diffusion image generation |
| `mind.cerebras` | Mind | Cerebras ultra-high-speed reasoning |
| `mind.deepseek` | Mind | DeepSeek reasoning engine |
| `mind.claude` | Mind | Claude/Anthropic reasoning engine |
| `mind.ollama` | Mind | Ollama local LLM |
| `vision.gaze_webcam` | Vision | Webcam gaze tracking |
| `vision.capture` | Vision | Image capture with Ollama + OCR |
| `voice.stt` | Voice | Speech-to-text (Whisper) |
| `output.avatar` | Output | VRM expression, idle behavior, and VOICEVOX TTS (Rust) |
| `io.discord` | I/O | Bidirectional Discord communication via MGP events (Rust) |

## Setup (all servers)

```bash
cd servers
python -m venv .venv

# Activate (Windows Git Bash)
source .venv/Scripts/activate
# Activate (Linux/macOS)
# source .venv/bin/activate

pip install --upgrade pip
for d in */; do [ -f "$d/pyproject.toml" ] && pip install "$d"; done
```

Or use the setup script:

```bash
bash scripts/setup-deps.sh
```

## Integration with ClotoCore

This repo is consumed by ClotoCore via `mcp.toml`'s `[paths]` section:

```toml
[paths]
servers = "/path/to/cloto-mcp-servers/servers"
```

Server args use `${servers}` variable expansion:

```toml
[[servers]]
id = "tool.terminal"
command = "python"
args = ["${servers}/terminal/server.py"]
```

## Testing

```bash
cd servers
python -m pytest tests/ -v
```

## License

This repository uses a dual-license model:

| Component | License |
|---|---|
| **CPersona** (`servers/cpersona/`) | MIT |
| **MGP Protocol** (`docs/MGP_*.md`) | MIT |
| All other servers and code | BSL 1.1 (converts to MIT on 2028-02-14) |

See [LICENSE](LICENSE) for the BSL 1.1 terms. CPersona and MGP are independently
MIT-licensed to enable adoption by any MCP host without restriction.

Contact: ClotoCore@proton.me
