# cloto-mcp-servers

MCP/MGP server collection for the [ClotoCore](https://github.com/Cloto-dev/ClotoCore) platform.

Extracted from the ClotoCore monorepo at commit `f9ea920`.

## Servers

| ID | Type | Description |
|----|------|-------------|
| `tool.terminal` | Tool | Sandboxed command execution |
| `tool.agent_utils` | Tool | Deterministic utilities (time, math, UUID, units, encode/decode) |
| `tool.cron` | Tool | CRON job management |
| `tool.embedding` | Tool | Vector embedding generation (ONNX + API providers) |
| `tool.websearch` | Tool | Web search integration (SearXNG, Tavily, DuckDuckGo) |
| `tool.research` | Tool | Research synthesis with search and LLM delegation |
| `tool.imagegen` | Tool | Stable Diffusion image generation |
| `mind.cerebras` | Mind | Cerebras ultra-high-speed reasoning |
| `mind.deepseek` | Mind | DeepSeek reasoning engine |
| `mind.claude` | Mind | Claude/Anthropic reasoning engine |
| `mind.ollama` | Mind | Ollama local LLM |
| `memory.cpersona` | Memory | CPersona persistent memory with FTS5 search (MIT) |
| `vision.gaze_webcam` | Vision | Webcam gaze tracking |
| `vision.capture` | Vision | Image capture with Ollama + OCR |
| `voice.stt` | Voice | Speech-to-text (Whisper) |

## Setup

```bash
# Create virtual environment
cd servers
python -m venv .venv

# Activate (Windows Git Bash)
source .venv/Scripts/activate
# Activate (Linux/macOS)
# source .venv/bin/activate

# Install dependencies
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
