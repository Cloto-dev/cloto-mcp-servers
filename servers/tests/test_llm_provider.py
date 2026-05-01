"""Tests for common.llm_provider reasoning-model fallback parser and prefill.

Covers the regression that shipped in this change:
- P1: Parse <tool_call> blocks out of reasoning_content when the upstream fails
  to populate the structured tool_calls[] channel (Qwen3 / DeepSeek-R1 quirk).
- P1.b: Surface stripped reasoning when both content and fallback parsing yield
  nothing, so the UI never renders an empty bubble.
- P2: Inject a </think> assistant prefill in iter 2+ when reasoning_think_prefill
  is enabled — and never inject it otherwise.
"""

import json
import os
import sys
from unittest.mock import AsyncMock, patch

import pytest

sys.path.insert(0, os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")))

import common.llm_provider as llm_provider
from common.llm_provider import (
    ProviderConfig,
    _extract_tool_calls_from_text,
    _model_suggests_reasoning,
    _strip_reasoning_artifacts,
    build_chat_messages,
    handle_think_with_tools,
    load_llm_provider_config,
    parse_chat_think_result,
)


@pytest.fixture
def config():
    return ProviderConfig(provider_id="test", model_id="test-model", display_name="Test")


@pytest.fixture
def config_prefill():
    return ProviderConfig(
        provider_id="test",
        model_id="test-model",
        display_name="Test",
        reasoning_think_prefill=True,
    )


# ---------------------------------------------------------------------------
# P1: _extract_tool_calls_from_text
# ---------------------------------------------------------------------------


def test_extract_hermes_xml_complete():
    text = (
        "<tool_call>\n"
        "<function=web_search>\n"
        "<parameter=query>hello</parameter>\n"
        "<parameter=max_results>5</parameter>\n"
        "</function>\n"
        "</tool_call>"
    )
    calls = _extract_tool_calls_from_text(text)
    assert len(calls) == 1
    assert calls[0]["name"] == "web_search"
    assert calls[0]["arguments"] == {"query": "hello", "max_results": "5"}
    assert calls[0]["id"].startswith("reasoning_fallback_")


def test_extract_openai_json_form():
    text = '<tool_call>\n{"name":"web_search","arguments":{"query":"hi"}}\n</tool_call>'
    calls = _extract_tool_calls_from_text(text)
    assert len(calls) == 1
    assert calls[0]["name"] == "web_search"
    assert calls[0]["arguments"] == {"query": "hi"}


def test_extract_truncated_no_closing_tag():
    """EOS-truncation: <tool_call> opens but never closes. Should still extract
    whatever fully-closed parameters are present."""
    text = (
        "<tool_call>\n"
        "<function=web_search>\n"
        "<parameter=language>ja</parameter>\n"
        "<parameter=query>ブルーアーカイブ 先生の秘"  # partial — no </parameter>
    )
    calls = _extract_tool_calls_from_text(text)
    assert len(calls) == 1
    assert calls[0]["name"] == "web_search"
    # Only the fully-closed parameter survives
    assert calls[0]["arguments"] == {"language": "ja"}


def test_extract_multiple_calls():
    text = (
        "<tool_call>\n<function=a>\n<parameter=x>1</parameter>\n</function>\n</tool_call>"
        "\nsome text\n"
        "<tool_call>\n<function=b>\n<parameter=y>2</parameter>\n</function>\n</tool_call>"
    )
    calls = _extract_tool_calls_from_text(text)
    assert [c["name"] for c in calls] == ["a", "b"]


def test_extract_empty_or_irrelevant_text():
    assert _extract_tool_calls_from_text("") == []
    assert _extract_tool_calls_from_text("plain reasoning, no tool call") == []
    # Body present but no <function=...> and not valid JSON → skipped
    assert _extract_tool_calls_from_text("<tool_call>garbage</tool_call>") == []


# ---------------------------------------------------------------------------
# P1.b: _strip_reasoning_artifacts
# ---------------------------------------------------------------------------


def test_strip_removes_think_tags():
    assert _strip_reasoning_artifacts("<think>hello</think>") == "hello"


def test_strip_cuts_trailing_tool_call():
    text = "Some reasoning.\n<tool_call>\n<function=x>partial"
    assert _strip_reasoning_artifacts(text) == "Some reasoning."


def test_strip_empty_input():
    assert _strip_reasoning_artifacts("") == ""
    assert _strip_reasoning_artifacts("   ") == ""


# ---------------------------------------------------------------------------
# P1/P1.b integration: parse_chat_think_result
# ---------------------------------------------------------------------------


def test_parse_structured_tool_calls_still_preferred(config):
    """Structured tool_calls[] path must keep working as before."""
    response = {
        "choices": [
            {
                "finish_reason": "tool_calls",
                "message": {
                    "content": "",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "function": {"name": "web_search", "arguments": '{"query":"x"}'},
                        }
                    ],
                },
            }
        ]
    }
    result = parse_chat_think_result(config, response)
    assert result["type"] == "tool_calls"
    assert result["calls"][0]["id"] == "call_1"
    assert result["calls"][0]["name"] == "web_search"


def test_parse_tool_calls_whitespace_content_falls_back_to_reasoning(config):
    """Qwen3 quirk: returns content='\\n\\n' with tool_calls while the substantive
    prose lives in reasoning_content. Whitespace-only content must not be
    promoted as assistant_content — otherwise the kernel emits a blank-label
    thinking step in the UI."""
    response = {
        "choices": [
            {
                "finish_reason": "tool_calls",
                "message": {
                    "content": "\n\n",
                    "reasoning_content": "Let me search the web for that.",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "function": {"name": "web_search", "arguments": '{"query":"x"}'},
                        }
                    ],
                },
            }
        ]
    }
    result = parse_chat_think_result(config, response)
    assert result["type"] == "tool_calls"
    # Substantive reasoning wins over whitespace-only content
    assert result["assistant_content"] == "Let me search the web for that."


def test_parse_tool_calls_no_reasoning_means_none_assistant_content(config):
    """When content is whitespace-only AND no reasoning_content is present,
    assistant_content must be None (not the whitespace, not empty string)."""
    response = {
        "choices": [
            {
                "finish_reason": "tool_calls",
                "message": {
                    "content": "   ",
                    "tool_calls": [
                        {
                            "id": "call_1",
                            "function": {"name": "web_search", "arguments": "{}"},
                        }
                    ],
                },
            }
        ]
    }
    result = parse_chat_think_result(config, response)
    assert result["type"] == "tool_calls"
    assert result["assistant_content"] is None


def test_parse_falls_back_to_reasoning_tool_call(config):
    """When tool_calls[] is empty but reasoning_content has <tool_call>, harvest it."""
    response = {
        "choices": [
            {
                "finish_reason": "stop",
                "message": {
                    "content": "",
                    "reasoning_content": (
                        "Let me search.\n"
                        "<tool_call>\n"
                        "<function=web_search>\n"
                        "<parameter=query>hi</parameter>\n"
                        "</function>\n"
                        "</tool_call>"
                    ),
                    "tool_calls": [],
                },
            }
        ]
    }
    result = parse_chat_think_result(config, response)
    assert result["type"] == "tool_calls"
    assert result["calls"][0]["name"] == "web_search"
    assert result["calls"][0]["arguments"] == {"query": "hi"}


def test_parse_empty_content_surfaces_stripped_reasoning(config):
    """P1.b: when no tool_call can be recovered, show reasoning instead of blank."""
    response = {
        "choices": [
            {
                "finish_reason": "stop",
                "message": {
                    "content": "",
                    "reasoning_content": "<think>Actually, the answer is 42.</think>",
                    "tool_calls": [],
                },
            }
        ]
    }
    result = parse_chat_think_result(config, response)
    assert result["type"] == "final"
    assert result["content"] == "Actually, the answer is 42."


def test_parse_genuine_empty_returns_empty_final(config):
    """When upstream truly gives us nothing, return empty final (and log)."""
    response = {
        "choices": [
            {
                "finish_reason": "stop",
                "message": {"content": "", "tool_calls": []},
            }
        ]
    }
    result = parse_chat_think_result(config, response)
    assert result == {"type": "final", "content": ""}


def test_parse_content_unchanged_when_non_empty(config):
    """Regression guard: normal content must pass through untouched."""
    response = {
        "choices": [
            {
                "finish_reason": "stop",
                "message": {"content": "Hello!", "reasoning_content": "some thinking"},
            }
        ]
    }
    result = parse_chat_think_result(config, response)
    assert result == {"type": "final", "content": "Hello!"}


# ---------------------------------------------------------------------------
# P2: prefill injection gating in handle_think_with_tools
# ---------------------------------------------------------------------------


def _minimal_args(tool_history=None):
    return {
        "agent": {"name": "tester", "description": "", "system_prompt": ""},
        "message": {"content": "hi", "timestamp": "2026-04-15T00:00:00Z"},
        "context": [],
        "tools": [],
        "tool_history": tool_history or [],
    }


@pytest.mark.asyncio
async def test_prefill_injected_when_flag_on_and_history_present(config_prefill):
    captured_messages: list[dict] = []

    async def fake_call(_cfg, messages, _tools):
        captured_messages.extend(messages)
        return {
            "choices": [{"finish_reason": "stop", "message": {"content": "ok"}}],
        }

    tool_history = [
        {"role": "assistant", "tool_calls": [{"id": "1", "function": {"name": "x", "arguments": "{}"}}]},
        {"role": "tool", "tool_call_id": "1", "content": "result"},
    ]
    with patch("common.llm_provider.call_llm_api", new=AsyncMock(side_effect=fake_call)):
        out = await handle_think_with_tools(config_prefill, _minimal_args(tool_history))

    assert captured_messages[-1] == {"role": "assistant", "content": "</think>\n\n"}
    payload = json.loads(out[0].text)
    assert payload["type"] == "final"


@pytest.mark.asyncio
async def test_prefill_not_injected_when_flag_off(config):
    captured_messages: list[dict] = []

    async def fake_call(_cfg, messages, _tools):
        captured_messages.extend(messages)
        return {"choices": [{"finish_reason": "stop", "message": {"content": "ok"}}]}

    tool_history = [{"role": "assistant", "content": "prior"}]
    with patch("common.llm_provider.call_llm_api", new=AsyncMock(side_effect=fake_call)):
        await handle_think_with_tools(config, _minimal_args(tool_history))

    assert all(msg.get("content") != "</think>\n\n" for msg in captured_messages)


@pytest.mark.asyncio
async def test_prefill_not_injected_on_iter_1(config_prefill):
    """iter 1 (no prior tool_history) must not get the prefill — it would confuse
    the model on the initial turn."""
    captured_messages: list[dict] = []

    async def fake_call(_cfg, messages, _tools):
        captured_messages.extend(messages)
        return {"choices": [{"finish_reason": "stop", "message": {"content": "ok"}}]}

    with patch("common.llm_provider.call_llm_api", new=AsyncMock(side_effect=fake_call)):
        await handle_think_with_tools(config_prefill, _minimal_args([]))

    assert all(msg.get("content") != "</think>\n\n" for msg in captured_messages)


# ---------------------------------------------------------------------------
# load_llm_provider_config: env-var plumbing for reasoning_think_prefill
# ---------------------------------------------------------------------------


def test_config_loader_respects_env_var_true(monkeypatch):
    monkeypatch.setenv("TESTP_REASONING_PREFILL", "true")
    cfg = load_llm_provider_config(prefix="TESTP", display_name="Test")
    assert cfg.reasoning_think_prefill is True


def test_config_loader_respects_env_var_false_overrides_default(monkeypatch):
    monkeypatch.setenv("TESTP_REASONING_PREFILL", "false")
    cfg = load_llm_provider_config(prefix="TESTP", display_name="Test", default_reasoning_prefill=True)
    assert cfg.reasoning_think_prefill is False


def test_config_loader_uses_default_when_env_var_absent(monkeypatch):
    monkeypatch.delenv("TESTP_REASONING_PREFILL", raising=False)
    monkeypatch.delenv("TESTP_MODEL", raising=False)
    cfg = load_llm_provider_config(prefix="TESTP", display_name="Test", default_reasoning_prefill=True)
    assert cfg.reasoning_think_prefill is True


# ---------------------------------------------------------------------------
# _model_suggests_reasoning heuristic
# ---------------------------------------------------------------------------


@pytest.mark.parametrize(
    "model_id,expected",
    [
        # Positive: recognisable reasoning / thinking families
        ("qwen/qwen3.5-9b", True),
        ("qwen3.5-4b", True),
        ("qwen3.6-35b-a3b", True),
        ("qwen3-instruct-7b", True),  # Qwen3 dual-mode wins over "instruct"
        ("qwen-3-7b", True),
        ("deepseek-r1", True),
        ("deepseek-r1-distill-llama-8b", True),
        ("deepseek-reasoner", True),
        ("qwq-32b", True),
        ("llama-3-thinking-70b", True),
        ("o1-preview", True),
        ("o1-mini", True),
        ("o3-mini", True),
        # Negative: known non-reasoning patterns
        ("qwen2.5-7b-instruct", False),
        ("llama-3.1-8b-instruct", False),
        ("mistral-7b-instruct-v0.2", False),
        # Unknown: neither hint matches → None (caller uses default)
        ("", None),
        ("gpt-4o", None),
        ("claude-sonnet-4-6", None),
        ("text-embedding-nomic-embed-text-v1.5", None),
        ("some-random-model-name", None),
    ],
)
def test_model_suggests_reasoning(model_id, expected):
    assert _model_suggests_reasoning(model_id) is expected


def test_config_loader_auto_detects_reasoning_from_model_id(monkeypatch):
    """When env var is absent, model_id like qwen3* flips prefill ON."""
    monkeypatch.delenv("TESTP_REASONING_PREFILL", raising=False)
    monkeypatch.setenv("TESTP_MODEL", "qwen/qwen3.5-9b")
    cfg = load_llm_provider_config(prefix="TESTP", display_name="Test", default_reasoning_prefill=False)
    assert cfg.reasoning_think_prefill is True


def test_config_loader_auto_detects_instruct_as_non_reasoning(monkeypatch):
    """An *-instruct model without reasoning hints flips prefill OFF."""
    monkeypatch.delenv("TESTP_REASONING_PREFILL", raising=False)
    monkeypatch.setenv("TESTP_MODEL", "qwen2.5-7b-instruct")
    # default=True simulates mind.local's current config; auto-detect overrides
    cfg = load_llm_provider_config(prefix="TESTP", display_name="Test", default_reasoning_prefill=True)
    assert cfg.reasoning_think_prefill is False


def test_config_loader_env_var_beats_auto_detect(monkeypatch):
    """User's explicit env var wins over heuristic."""
    monkeypatch.setenv("TESTP_REASONING_PREFILL", "false")
    monkeypatch.setenv("TESTP_MODEL", "qwen/qwen3.5-9b")  # would auto-detect True
    cfg = load_llm_provider_config(prefix="TESTP", display_name="Test", default_reasoning_prefill=True)
    assert cfg.reasoning_think_prefill is False


def test_config_loader_unknown_model_falls_back_to_default(monkeypatch):
    """Unknown model name leaves the server-supplied default untouched."""
    monkeypatch.delenv("TESTP_REASONING_PREFILL", raising=False)
    monkeypatch.setenv("TESTP_MODEL", "gpt-4o")
    cfg = load_llm_provider_config(prefix="TESTP", display_name="Test", default_reasoning_prefill=True)
    assert cfg.reasoning_think_prefill is True


# ── build_chat_messages (xml_user_prefix mode, v2.4.13) ──

_AGENT = {"id": "agent.test", "name": "Test", "description": "test agent", "metadata": {}}
_MSG = {"content": "hello", "source": {"type": "User", "name": "Taro"}, "metadata": {}}


def _mem(content, ts="2026-04-01T10:00:00+09:00", source_type="User"):
    return {
        "content": content,
        "timestamp": ts,
        "source": {"type": source_type, "name": "Taro"},
    }


def _conv(content):
    return {"content": content, "source": {"type": "User"}, "context_type": "conversation"}


def test_bcm_xml_mode_no_memory(monkeypatch):
    """No memories → user message has no XML block."""
    monkeypatch.setattr(llm_provider, "_MEMORY_INJECTION_MODE", "xml_user_prefix")
    msgs = build_chat_messages(_AGENT, _MSG, context=[])
    user = msgs[-1]
    assert user["role"] == "user"
    assert "<background_memories>" not in user["content"]


def test_bcm_xml_mode_injects_block(monkeypatch):
    """Memories present → user content contains <background_memories> block."""
    monkeypatch.setattr(llm_provider, "_MEMORY_INJECTION_MODE", "xml_user_prefix")
    msgs = build_chat_messages(_AGENT, _MSG, context=[_mem("パンを食べた")])
    user = msgs[-1]
    assert user["role"] == "user"
    assert "<background_memories>" in user["content"]
    assert "パンを食べた" in user["content"]
    assert "</background_memories>" in user["content"]


def test_bcm_xml_mode_no_standalone_memory_turns(monkeypatch):
    """XML mode: no intermediate role=user/assistant turns from memories."""
    monkeypatch.setattr(llm_provider, "_MEMORY_INJECTION_MODE", "xml_user_prefix")
    msgs = build_chat_messages(_AGENT, _MSG, context=[_mem("memory1"), _mem("memory2")])
    # Only the final user message should have role=user (from the actual message)
    user_turns = [m for m in msgs[1:] if m["role"] == "user"]
    assert len(user_turns) == 1
    assert "<background_memories>" in user_turns[0]["content"]


def test_bcm_xml_mode_timestamp_in_block(monkeypatch):
    """Timestamp is embedded inside the XML block."""
    monkeypatch.setattr(llm_provider, "_MEMORY_INJECTION_MODE", "xml_user_prefix")
    msgs = build_chat_messages(_AGENT, _MSG, context=[_mem("content", ts="2026-04-01T10:00:00+09:00")])
    user_content = msgs[-1]["content"]
    assert "2026-04-01" in user_content


def test_bcm_xml_mode_agent_labeled(monkeypatch):
    """Agent-source memory gets [agent] label inside the block."""
    monkeypatch.setattr(llm_provider, "_MEMORY_INJECTION_MODE", "xml_user_prefix")
    msgs = build_chat_messages(_AGENT, _MSG, context=[_mem("bot reply", source_type="Agent")])
    user_content = msgs[-1]["content"]
    assert "[agent]" in user_content


def test_bcm_xml_mode_user_content_appended(monkeypatch):
    """Actual user message content follows the XML block after two newlines."""
    monkeypatch.setattr(llm_provider, "_MEMORY_INJECTION_MODE", "xml_user_prefix")
    msgs = build_chat_messages(_AGENT, _MSG, context=[_mem("memory")])
    user_content = msgs[-1]["content"]
    assert "</background_memories>\n\n" in user_content
    assert user_content.endswith("hello")


def test_bcm_chat_mode_legacy_turns(monkeypatch):
    """chat mode: intermediate role=user/assistant turns appear (legacy behaviour)."""
    monkeypatch.setattr(llm_provider, "_MEMORY_INJECTION_MODE", "chat")
    msgs = build_chat_messages(_AGENT, _MSG, context=[_mem("パンを食べた")])
    roles = [m["role"] for m in msgs]
    # Should have a user turn for the memory before the final user message
    assert roles.count("user") >= 2


def test_bcm_conversation_msgs_unchanged(monkeypatch):
    """context_type=conversation messages stay as chat turns in xml mode."""
    monkeypatch.setattr(llm_provider, "_MEMORY_INJECTION_MODE", "xml_user_prefix")
    msgs = build_chat_messages(_AGENT, _MSG, context=[_conv("recent channel msg")])
    # Conversation turn should appear as a chat turn, not in the user XML block
    conv_turns = [m for m in msgs if m.get("content") == "recent channel msg"]
    assert len(conv_turns) == 1
    assert conv_turns[0]["role"] == "user"


def test_bcm_xml_no_memory_no_injection(monkeypatch):
    """xml mode with empty context → user message is clean (no XML prefix)."""
    monkeypatch.setattr(llm_provider, "_MEMORY_INJECTION_MODE", "xml_user_prefix")
    msgs = build_chat_messages(_AGENT, _MSG, context=[])
    user = msgs[-1]
    assert user["content"] == "[Taro]: hello"
