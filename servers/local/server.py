"""
Cloto MCP Server: Local LLM (recommended)
OpenAI-compatible wrapper for any local LLM server (LM Studio, llama.cpp's
llama-server, vLLM, Ollama's /v1/*, etc.). Default target is LM Studio on
port 1234. Users select the loaded model name via Dashboard Settings or the
LOCAL_MODEL env var (no hardcoded default).
"""

import asyncio
import os
import sys

_script_dir = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.normpath(os.path.join(_script_dir, "..")))

from common.llm_provider import create_llm_mcp_server, load_llm_provider_config, run_server

config = load_llm_provider_config(
    prefix="LOCAL",
    display_name="Local LLM",
    default_model="",
    supports_tools=True,
    default_timeout=120,
    # LM Studio commonly hosts Qwen3 / DeepSeek-R1 style reasoning models whose
    # iter-2 tool calls leak into the <think> block. Default on; override via
    # LOCAL_REASONING_PREFILL=false if a non-reasoning model is loaded.
    default_reasoning_prefill=True,
)

server = create_llm_mcp_server(config)

if __name__ == "__main__":
    asyncio.run(run_server(server, config))
