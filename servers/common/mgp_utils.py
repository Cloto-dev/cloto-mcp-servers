"""
MGP (Multi-Agent Gateway Protocol) capability helpers.

Lightweight utilities for declaring MGP capabilities in MCP server
initialize responses. Not a full SDK — for comprehensive MGP features
(events, streaming, callbacks), implement the JSON-RPC methods directly.

See: docs/MGP_SPEC.md, docs/MGP_GUIDE.md
"""

from __future__ import annotations

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
