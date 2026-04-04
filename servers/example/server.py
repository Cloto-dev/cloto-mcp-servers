"""
Example MGP Server for ClotoCore
=================================

A minimal reference implementation showing how to build an MCP server
with MGP extensions for ClotoCore. Use this as a starting point for
your own servers.

Features demonstrated:
- ToolRegistry pattern (eliminates list_tools/call_tool boilerplate)
- MGP capability declaration (permissions_required, trust_level)
- Tool annotations (destructiveHint, readOnlyHint)
- Async tool handlers with validation

Run:
    python servers/example/server.py

Register in mcp.toml:
    [[servers]]
    id = "tool.example"
    display_name = "Example"
    command = "python"
    args = ["${servers}/example/server.py"]
    transport = "stdio"
    [servers.mgp]
    trust_level = "standard"
"""

import asyncio
import os
import sys
from datetime import datetime, timezone

# ── Path setup (required when running inside cloto-mcp-servers) ──
sys.path.insert(0, os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")))

from mcp.types import ToolAnnotations

from common.mcp_utils import ToolRegistry, run_mcp_server

# ============================================================
# Server Setup
# ============================================================

registry = ToolRegistry("mgp-example")

# ============================================================
# Tool 1: A simple read-only tool
# ============================================================

GREETING_SCHEMA = {
    "type": "object",
    "properties": {
        "name": {
            "type": "string",
            "description": "Name of the person to greet",
        },
    },
    "required": ["name"],
}


@registry.tool(
    name="greet",
    description="Return a friendly greeting. Safe, read-only, no side effects.",
    schema=GREETING_SCHEMA,
)
async def greet(arguments: dict) -> dict:
    name = arguments.get("name", "World")
    now = datetime.now(tz=timezone.utc).strftime("%H:%M UTC")
    return {"greeting": f"Hello, {name}! The time is {now}."}


# ============================================================
# Tool 2: A tool with auto-validated parameters
# ============================================================

REPEAT_SCHEMA = {
    "type": "object",
    "properties": {
        "text": {"type": "string", "description": "Text to repeat"},
        "count": {"type": "integer", "description": "How many times (1-10)", "default": 3},
    },
    "required": ["text"],
}


async def _repeat_handler(text: str, count: int) -> dict:
    """Repeat text N times. Demonstrates auto_tool with type validation."""
    return {"result": " ".join([text] * count)}


# auto_tool extracts and validates parameters automatically.
# params: list of (key, type) or (key, type, default)
registry.auto_tool(
    name="repeat",
    description="Repeat a text string N times",
    schema=REPEAT_SCHEMA,
    handler=_repeat_handler,
    params=[("text", str), ("count", int, 3)],
    # Mark as read-only — ClotoCore uses this for security metadata
    annotations=ToolAnnotations(readOnlyHint=True),
)

# ============================================================
# Tool 3: A destructive tool (requires HITL approval in ClotoCore)
# ============================================================

DELETE_SCHEMA = {
    "type": "object",
    "properties": {
        "item_id": {"type": "string", "description": "ID of the item to delete"},
    },
    "required": ["item_id"],
}

registry.auto_tool(
    name="delete_item",
    description="Delete an item by ID. This is a destructive operation.",
    schema=DELETE_SCHEMA,
    handler=lambda item_id: {"deleted": item_id, "note": "This is a demo — nothing was actually deleted"},
    params=[("item_id", str)],
    # destructiveHint=True triggers ClotoCore's HITL approval gate (L12)
    annotations=ToolAnnotations(destructiveHint=True),
)

# ============================================================
# MGP Capability Declaration
# ============================================================
# ClotoCore reads the `mgp` object from the initialize response.
# This is handled by overriding the server's initialization options.
#
# For servers using ToolRegistry (which wraps mcp.Server), the MGP
# capabilities are declared in mcp.toml [servers.mgp] on the kernel
# side. The server can also self-declare in the initialize response
# by customizing the Server class — see the avatar and discord
# servers (Rust) for examples.
#
# For Python servers, the recommended approach is:
#   1. Declare permissions in mcp.toml: required_permissions = ["network.outbound"]
#   2. Set trust_level in mcp.toml: [servers.mgp] trust_level = "standard"
#
# The kernel merges both sources (config + server declaration).

# ============================================================
# Entry Point
# ============================================================

if __name__ == "__main__":
    asyncio.run(run_mcp_server(registry))
