"""
MGP (Multi-Agent Gateway Protocol) capability helpers.

Lightweight utilities for declaring MGP capabilities in MCP server
initialize responses. Not a full SDK — for comprehensive MGP features
(events, streaming, callbacks), implement the JSON-RPC methods directly.

See: docs/MGP_SPEC.md, docs/MGP_GUIDE.md
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Any

if TYPE_CHECKING:
    from mcp.server.session import ServerSession

MGP_VERSION = "0.6.0"


class MgpCapabilities:
    """Builder for MGP capability declarations in initialize responses.

    Usage::

        mgp = MgpCapabilities()
        mgp.require_permission("network.outbound")
        mgp.set_trust_level("standard")

        # In your initialize handler:
        capabilities = {"tools": {}, **mgp.as_dict()}
    """

    def __init__(self, version: str = MGP_VERSION):
        self._version = version
        self._extensions: list[str] = ["permissions"]
        self._permissions: list[str] = []
        self._trust_level: str | None = None
        self._server_id: str | None = None

    def require_permission(self, permission: str) -> MgpCapabilities:
        """Declare a required permission (e.g., 'network.outbound', 'filesystem.write').

        The kernel will gate server startup on operator approval for these
        permissions (or auto-approve in YOLO mode, subject to exceptions).
        """
        if permission not in self._permissions:
            self._permissions.append(permission)
        return self

    def set_trust_level(self, level: str) -> MgpCapabilities:
        """Self-declare trust level (informational — kernel config overrides).

        Valid levels: 'core', 'standard', 'experimental', 'untrusted'.
        """
        self._trust_level = level
        return self

    def set_server_id(self, server_id: str) -> MgpCapabilities:
        """Set a unique server identifier."""
        self._server_id = server_id
        return self

    def add_extension(self, extension: str) -> MgpCapabilities:
        """Declare support for an additional MGP extension.

        Common extensions: 'permissions', 'tool_security', 'lifecycle',
        'streaming', 'events', 'callbacks', 'discovery'.
        """
        if extension not in self._extensions:
            self._extensions.append(extension)
        return self

    def as_dict(self) -> dict:
        """Return the MGP capabilities as a dict for merging into initialize response.

        Returns::

            {"mgp": {"version": "0.6.0", "extensions": [...], ...}}
        """
        mgp: dict = {
            "version": self._version,
            "extensions": self._extensions,
        }
        if self._permissions:
            mgp["permissions_required"] = self._permissions
        if self._trust_level:
            mgp["trust_level"] = self._trust_level
        if self._server_id:
            mgp["server_id"] = self._server_id
        return {"mgp": mgp}


async def send_mgp_stream_chunk(
    session: "ServerSession",
    request_id: int | str,
    index: int,
    content: dict,
    done: bool = False,
    mgp_meta: dict | None = None,
) -> None:
    """Emit a ``notifications/mgp.stream.chunk`` (MGP §12.4).

    MGP defines custom JSON-RPC notification methods that are not part of the
    MCP-standard ``ServerNotificationType`` closed union, so the typed
    ``session.send_notification()`` helper cannot be used. We construct the
    ``JSONRPCNotification`` directly and feed it to the session's write stream.

    This is technically reaching into a private attribute (``_write_stream``);
    if/when the MCP SDK exposes a public API for arbitrary notifications,
    replace this implementation without changing the call sites.

    Parameters
    ----------
    session:
        ``ServerSession`` instance, obtained via ``server.request_context.session``.
    request_id:
        JSON-RPC id of the originating ``tools/call`` — used by the kernel to
        route the chunk to the correct stream collector (see
        ``managers/mcp_client.rs::call_tool_streaming``).
    index:
        Zero-based monotonically increasing chunk index per request. Used for
        gap detection (MGP §12.9).
    content:
        Chunk payload, typically ``{"type": "text", "text": "..."}``.
    done:
        ``True`` only on the final chunk (informational; the authoritative
        terminator is the ``tools/call`` response itself, MGP §12.5).
    mgp_meta:
        Optional ``_mgp`` sub-object for chunk-level metadata.
    """
    from mcp.shared.message import SessionMessage
    from mcp.types import JSONRPCMessage, JSONRPCNotification

    params: dict[str, Any] = {
        "request_id": request_id,
        "index": index,
        "content": content,
        "done": done,
    }
    if mgp_meta:
        params["_mgp"] = mgp_meta

    notification = JSONRPCNotification(
        jsonrpc="2.0",
        method="notifications/mgp.stream.chunk",
        params=params,
    )
    await session._write_stream.send(SessionMessage(message=JSONRPCMessage(notification)))
