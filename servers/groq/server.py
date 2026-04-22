"""
Cloto MCP Server: Groq
OpenAI-compatible ultra-fast LLM inference (gpt-oss-120b) via MCP protocol.
"""

import asyncio
import os
import sys

_script_dir = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.normpath(os.path.join(_script_dir, "..")))

from common.llm_provider import create_llm_mcp_server, load_llm_provider_config, run_server

config = load_llm_provider_config(
    prefix="GROQ",
    display_name="Groq",
    default_model="openai/gpt-oss-120b",
    supports_tools=True,
    default_timeout=30,
)

server = create_llm_mcp_server(config)

if __name__ == "__main__":
    asyncio.run(run_server(server, config))
