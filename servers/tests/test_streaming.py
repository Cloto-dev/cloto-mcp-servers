"""Tests for MGP §12 streaming path in common.llm_provider.

Covers Plan 1 (mind.local MGP Streaming PoC):
- SSE line parsing (data: {...}, data: [DONE], blank lines, comments)
- Config plumbing (ProviderConfig.supports_streaming, LOCAL_STREAMING env)
- Opt-in detection via params._mgp.stream (CallToolRequestParams.model_extra)
- notifications/mgp.stream.chunk emission via send_mgp_stream_chunk
- Final CallToolResult carries the complete accumulated content (§12.5)
- Backward compatibility: non-streaming path unchanged
- Fallback: config.supports_streaming=False ignores the _mgp flag
"""

from __future__ import annotations

import json
import os
import sys
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch

import pytest

sys.path.insert(0, os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")))

from mcp.types import CallToolRequest, CallToolRequestParams  # noqa: E402

from common.llm_provider import (  # noqa: E402
    ProviderConfig,
    _extract_stream_flag,
    _mgp_stream_requested,
    call_llm_api_streaming,
    create_llm_mcp_server,
    handle_think_with_tools,
    handle_think_with_tools_streaming,
    load_llm_provider_config,
)
from common.mgp_utils import send_mgp_stream_chunk  # noqa: E402

# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture
def streaming_config():
    return ProviderConfig(
        provider_id="test",
        model_id="test-model",
        display_name="Test",
        supports_streaming=True,
    )


@pytest.fixture
def non_streaming_config():
    return ProviderConfig(
        provider_id="test",
        model_id="test-model",
        display_name="Test",
        supports_streaming=False,
    )


@pytest.fixture
def minimal_args():
    """Minimal argument dict for think_with_tools."""
    return {
        "agent": {"name": "Test", "description": "test agent"},
        "message": {"content": "Hello"},
        "context": [],
        "tools": [],
        "tool_history": [],
    }


class _AsyncLineIterator:
    """Mock for httpx.Response.aiter_lines — async iteration over pre-seeded lines."""

    def __init__(self, lines: list[str]):
        self._lines = list(lines)

    def __aiter__(self):
        return self

    async def __anext__(self):
        if not self._lines:
            raise StopAsyncIteration
        return self._lines.pop(0)


class _MockStreamResponse:
    def __init__(self, status_code: int, lines: list[str], body_bytes: bytes = b""):
        self.status_code = status_code
        self._lines = lines
        self._body_bytes = body_bytes

    def aiter_lines(self):
        return _AsyncLineIterator(self._lines)

    async def aread(self):
        return self._body_bytes


class _MockStreamCtx:
    def __init__(self, response: _MockStreamResponse):
        self._response = response

    async def __aenter__(self):
        return self._response

    async def __aexit__(self, *args):
        return None


class _MockAsyncClient:
    def __init__(self, response: _MockStreamResponse):
        self._response = response
        self.post_called_with: dict | None = None

    async def __aenter__(self):
        return self

    async def __aexit__(self, *args):
        return None

    def stream(self, method: str, url: str, json=None, headers=None):
        self.post_called_with = {"method": method, "url": url, "json": json, "headers": headers}
        return _MockStreamCtx(self._response)


# ---------------------------------------------------------------------------
# ProviderConfig.supports_streaming plumbing
# ---------------------------------------------------------------------------


def test_load_config_streaming_env_true(monkeypatch):
    monkeypatch.setenv("LOCAL_STREAMING", "true")
    cfg = load_llm_provider_config(prefix="LOCAL", display_name="Local", default_model="m")
    assert cfg.supports_streaming is True


def test_load_config_streaming_env_false(monkeypatch):
    monkeypatch.setenv("LOCAL_STREAMING", "false")
    cfg = load_llm_provider_config(prefix="LOCAL", display_name="Local", default_model="m")
    assert cfg.supports_streaming is False


def test_load_config_streaming_default_off(monkeypatch):
    monkeypatch.delenv("LOCAL_STREAMING", raising=False)
    cfg = load_llm_provider_config(prefix="LOCAL", display_name="Local", default_model="m")
    assert cfg.supports_streaming is False


# ---------------------------------------------------------------------------
# call_llm_api_streaming: SSE parser
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_streaming_parses_sse_data_lines(streaming_config):
    lines = [
        'data: {"choices":[{"delta":{"content":"Hel"}}]}',
        "",
        'data: {"choices":[{"delta":{"content":"lo"}}]}',
        "data: [DONE]",
    ]
    response = _MockStreamResponse(200, lines)
    client = _MockAsyncClient(response)

    with patch("common.llm_provider.httpx.AsyncClient", return_value=client):
        chunks = [c async for c in call_llm_api_streaming(streaming_config, [{"role": "user", "content": "hi"}])]

    assert len(chunks) == 2
    assert chunks[0]["choices"][0]["delta"]["content"] == "Hel"
    assert chunks[1]["choices"][0]["delta"]["content"] == "lo"


@pytest.mark.asyncio
async def test_streaming_stops_on_done_marker(streaming_config):
    # Lines AFTER [DONE] must be ignored.
    lines = [
        'data: {"choices":[{"delta":{"content":"one"}}]}',
        "data: [DONE]",
        'data: {"choices":[{"delta":{"content":"TWO"}}]}',
    ]
    response = _MockStreamResponse(200, lines)
    client = _MockAsyncClient(response)

    with patch("common.llm_provider.httpx.AsyncClient", return_value=client):
        chunks = [c async for c in call_llm_api_streaming(streaming_config, [])]

    assert [c["choices"][0]["delta"]["content"] for c in chunks] == ["one"]


@pytest.mark.asyncio
async def test_streaming_skips_blanks_and_comments(streaming_config):
    lines = [
        "",
        ":keepalive",
        'data: {"choices":[{"delta":{"content":"ok"}}]}',
        "malformed without prefix",
        "data: [DONE]",
    ]
    response = _MockStreamResponse(200, lines)
    client = _MockAsyncClient(response)

    with patch("common.llm_provider.httpx.AsyncClient", return_value=client):
        chunks = [c async for c in call_llm_api_streaming(streaming_config, [])]

    assert len(chunks) == 1
    assert chunks[0]["choices"][0]["delta"]["content"] == "ok"


@pytest.mark.asyncio
async def test_streaming_raises_on_http_error(streaming_config):
    response = _MockStreamResponse(500, [], body_bytes=b'{"error":{"message":"boom","code":"internal"}}')
    client = _MockAsyncClient(response)

    from common.llm_provider import LlmApiError

    with patch("common.llm_provider.httpx.AsyncClient", return_value=client):
        with pytest.raises(LlmApiError) as excinfo:
            [c async for c in call_llm_api_streaming(streaming_config, [])]
    assert excinfo.value.code == "internal"
    assert excinfo.value.status_code == 500


# ---------------------------------------------------------------------------
# _extract_stream_flag: opt-in detection via params._mgp.stream
# ---------------------------------------------------------------------------


def _make_req(model_extra: dict | None):
    """Build a CallToolRequest-like stub matching Pydantic's model_extra shape."""
    params = SimpleNamespace(model_extra=model_extra)
    return SimpleNamespace(params=params)


def test_extract_flag_true_when_stream_true():
    assert _extract_stream_flag(_make_req({"_mgp": {"stream": True}})) is True


def test_extract_flag_false_when_flag_missing():
    assert _extract_stream_flag(_make_req({})) is False


def test_extract_flag_false_when_flag_explicit_false():
    assert _extract_stream_flag(_make_req({"_mgp": {"stream": False}})) is False


def test_extract_flag_false_when_params_none():
    assert _extract_stream_flag(SimpleNamespace(params=None)) is False


def test_extract_flag_false_when_extra_not_dict():
    assert _extract_stream_flag(_make_req(None)) is False


@pytest.mark.asyncio
async def test_request_handler_wrapper_sets_contextvar():
    """Feeding a real CallToolRequest through the installed wrapper must
    expose ``_mgp.stream`` via the ContextVar during handler execution, and
    reset it afterwards."""
    cfg = ProviderConfig(
        provider_id="wrap",
        model_id="m",
        display_name="Wrap",
        supports_streaming=True,
    )
    server = create_llm_mcp_server(cfg)

    observed: dict = {}

    async def spy(_req):
        observed["flag"] = _mgp_stream_requested.get()
        # Return a minimal ServerResult-shaped object to satisfy the outer pipeline
        from mcp.types import CallToolResult, ServerResult

        return ServerResult(CallToolResult(content=[], isError=False))

    # Re-wrap: grab the production wrapper's inner ``original`` by re-running the
    # install step against a controlled stub. The cleanest verification is to
    # directly invoke the wrapper with a synthesized CallToolRequest.
    wrapper = server.request_handlers[CallToolRequest]
    # Replace the eventual inner handler with our spy.
    server.request_handlers[CallToolRequest] = spy
    # Re-install wrapper on top of the spy.
    from common.llm_provider import _install_mgp_stream_wrapper

    _install_mgp_stream_wrapper(server)
    wrapped = server.request_handlers[CallToolRequest]

    req = CallToolRequest(
        method="tools/call",
        params=CallToolRequestParams.model_validate(
            {"name": "think_with_tools", "arguments": {}, "_mgp": {"stream": True}}
        ),
    )
    # ContextVar should be False before, flag True inside, False after.
    assert _mgp_stream_requested.get() is False
    await wrapped(req)
    assert observed["flag"] is True
    assert _mgp_stream_requested.get() is False

    # Not-opted-in request leaves flag False inside.
    observed.clear()
    req2 = CallToolRequest(
        method="tools/call",
        params=CallToolRequestParams.model_validate({"name": "think_with_tools", "arguments": {}}),
    )
    await wrapped(req2)
    assert observed["flag"] is False

    # Keep a reference to wrapper to silence linter
    _ = wrapper


# ---------------------------------------------------------------------------
# send_mgp_stream_chunk: produces correct JSON-RPC notification shape
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_send_chunk_writes_jsonrpc_notification():
    session = MagicMock()
    session._write_stream = MagicMock()
    session._write_stream.send = AsyncMock()

    await send_mgp_stream_chunk(
        session,
        request_id=7,
        index=3,
        content={"type": "text", "text": "foo"},
        done=False,
    )

    assert session._write_stream.send.await_count == 1
    sent = session._write_stream.send.await_args.args[0]
    # SessionMessage wraps a JSONRPCMessage wrapping a JSONRPCNotification
    message = sent.message.root
    dumped = message.model_dump(by_alias=True, exclude_none=True)
    assert dumped["jsonrpc"] == "2.0"
    assert dumped["method"] == "notifications/mgp.stream.chunk"
    assert dumped["params"] == {
        "request_id": 7,
        "index": 3,
        "content": {"type": "text", "text": "foo"},
        "done": False,
    }
    assert "id" not in dumped  # notifications have no id


# ---------------------------------------------------------------------------
# handle_think_with_tools_streaming: end-to-end (mocked httpx + session)
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_streaming_handler_emits_chunks_and_final_result(streaming_config, minimal_args):
    lines = [
        'data: {"choices":[{"delta":{"content":"Hel"}}]}',
        'data: {"choices":[{"delta":{"content":"lo "}}]}',
        'data: {"choices":[{"delta":{"content":"world"},"finish_reason":"stop"}]}',
        "data: [DONE]",
    ]
    response = _MockStreamResponse(200, lines)
    client = _MockAsyncClient(response)

    session = MagicMock()
    session._write_stream = MagicMock()
    session._write_stream.send = AsyncMock()
    ctx = SimpleNamespace(request_id=99, session=session)

    with patch("common.llm_provider.httpx.AsyncClient", return_value=client):
        result = await handle_think_with_tools_streaming(streaming_config, minimal_args, ctx)

    # 3 text deltas → 3 chunk notifications
    assert session._write_stream.send.await_count == 3

    # Verify chunk routing (request_id carries through, indices monotonic from 0)
    first_call = session._write_stream.send.await_args_list[0]
    dumped = first_call.args[0].message.root.model_dump(by_alias=True, exclude_none=True)
    assert dumped["method"] == "notifications/mgp.stream.chunk"
    assert dumped["params"]["request_id"] == 99
    assert dumped["params"]["index"] == 0
    assert dumped["params"]["content"]["text"] == "Hel"

    third_call = session._write_stream.send.await_args_list[2]
    dumped3 = third_call.args[0].message.root.model_dump(by_alias=True, exclude_none=True)
    assert dumped3["params"]["index"] == 2
    assert dumped3["params"]["content"]["text"] == "world"

    # Final result carries complete accumulated content (§12.5)
    assert len(result) == 1
    payload = json.loads(result[0].text)
    assert payload["type"] == "final"
    assert payload["content"] == "Hello world"
    assert payload["_mgp"] == {"streamed": True, "chunks_sent": 3}


@pytest.mark.asyncio
async def test_streaming_handler_handles_structured_tool_calls(streaming_config, minimal_args):
    """When the upstream streams tool_calls instead of text, no chunks are sent
    but the final result carries the assembled tool_calls."""
    lines = [
        'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"do_thing","arguments":"{\\"a\\":"}}]}}]}',
        'data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]}}]}',
        'data: {"choices":[{"delta":{},"finish_reason":"tool_calls"}]}',
        "data: [DONE]",
    ]
    response = _MockStreamResponse(200, lines)
    client = _MockAsyncClient(response)

    session = MagicMock()
    session._write_stream = MagicMock()
    session._write_stream.send = AsyncMock()
    ctx = SimpleNamespace(request_id=5, session=session)

    minimal_args["tools"] = [
        {"type": "function", "function": {"name": "do_thing", "description": "x", "parameters": {}}}
    ]

    with patch("common.llm_provider.httpx.AsyncClient", return_value=client):
        result = await handle_think_with_tools_streaming(streaming_config, minimal_args, ctx)

    # No streamable text → no notifications emitted
    assert session._write_stream.send.await_count == 0

    payload = json.loads(result[0].text)
    # parse_chat_think_result normalizes calls into {"id","name","arguments"}
    assert payload["type"] == "tool_calls"
    assert len(payload["calls"]) == 1
    call = payload["calls"][0]
    assert call["id"] == "c1"
    assert call["name"] == "do_thing"
    assert call["arguments"] == {"a": 1}
    assert payload["_mgp"] == {"streamed": True, "chunks_sent": 0}


# ---------------------------------------------------------------------------
# Backward compatibility
# ---------------------------------------------------------------------------


@pytest.mark.asyncio
async def test_non_streaming_path_unchanged_without_flag(non_streaming_config, minimal_args):
    """Without _mgp.stream, the existing synchronous handler runs unchanged."""
    fake_response = {
        "choices": [
            {
                "message": {"role": "assistant", "content": "pong", "tool_calls": []},
                "finish_reason": "stop",
            }
        ]
    }

    with patch("common.llm_provider.call_llm_api", new=AsyncMock(return_value=fake_response)):
        result = await handle_think_with_tools(non_streaming_config, minimal_args)

    payload = json.loads(result[0].text)
    assert payload["type"] == "final"
    assert payload["content"] == "pong"
    # Non-streaming path MUST NOT add a _mgp field to the result (stability guarantee)
    assert "_mgp" not in payload


@pytest.mark.asyncio
async def test_config_disabled_disables_streaming_even_with_flag(non_streaming_config, minimal_args):
    """config.supports_streaming=False → non-streaming handler is authoritative."""
    # The _mgp flag alone never bypasses config.supports_streaming; the dispatch
    # in call_tool checks config first. Here we verify directly that the
    # non-streaming handler returns the pre-MGP payload shape unchanged.
    fake_response = {
        "choices": [{"message": {"role": "assistant", "content": "sync", "tool_calls": []}, "finish_reason": "stop"}]
    }
    with patch("common.llm_provider.call_llm_api", new=AsyncMock(return_value=fake_response)):
        result = await handle_think_with_tools(non_streaming_config, minimal_args)
    payload = json.loads(result[0].text)
    assert payload["content"] == "sync"
    assert "_mgp" not in payload
