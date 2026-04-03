# Discord Context Search Design (v0.5.0 scope)

## Status: Draft (2026-04-03)

Discussion record for the conversation context overhaul planned for Discord Bridge v0.5.0.

---

## Problem Statement

Discord Bridge v0.4.x fetches the last N messages (default 15) from the channel via Discord API and injects them as `conversation_context` into the LLM prompt. This causes:

1. **Context contamination**: The LLM responds to messages not directed at the bot (e.g., other users' off-topic messages leak through bot replies in the context)
2. **Runaway behavior**: Combined with the (now-fixed in v0.4.2) webhook/bot mention bypass, the bot would enter feedback loops
3. **Soft constraint failure**: The LLM system prompt says "ignore unless directly relevant" but this is unreliable -- the LLM often picks up interesting context and responds to it

### Current Data Flow

```
Bridge → Discord API (last 15 messages) → speaker filter → callback payload
  → Kernel (events.rs) → system.rs merges with CPersona recall
  → llm_provider.py injects as context_type="conversation"
  → LLM prompt with soft "background only" framing
```

### Root Cause

Blind time-window fetching assumes "recent = relevant", which is false in active multi-user channels.

---

## Proposed Architecture: Relevance-Judged Context

Replace blind fetching with a 2-layer system where each message is evaluated for relevance before inclusion.

### Layer 1: Immediate Fetch (Bridge-side)

- Fetch **5 messages** from Discord API (reduced from 15)
- Apply existing per-user speaker filter (unchanged)
- These 5 candidates are NOT yet included in the payload

### Layer 2: Relevance Judgment (Embedding Server)

The bridge sends the **triggering message** (anchor) and the **5 candidates** to the embedding server for cosine similarity analysis.

```
Bridge(Rust) --HTTP POST--> embedding:8401/judge_relevance
```

Each candidate receives a 3-tier verdict:

| Verdict | Condition | Action |
|---------|-----------|--------|
| `relevant` | score >= T_high | Include in context |
| `irrelevant` | score <= T_low | Exclude |
| `investigate` | T_low < score < T_high | Trigger expanded fetch |

### Layer 2b: Expanded Fetch (Conditional)

If any candidate is `investigate`:
1. Fetch additional 15-20 messages from Discord API
2. Re-evaluate expanded set against the anchor
3. **One-shot only** -- no recursive expansion. Second pass forces all remaining `investigate` to `irrelevant`

### Final Output

Only `relevant` messages are included in the callback payload's `conversation_context`.

---

## Embedding Server: `judge_relevance` Endpoint

New HTTP endpoint + MCP tool on the embedding server.

### HTTP API

```
POST /judge_relevance
{
  "anchor": "string (the triggering message text)",
  "candidates": [
    {"id": "msg-123", "text": "candidate message text"},
    ...
  ],
  "threshold_high": 0.6,
  "threshold_low": 0.3
}

Response:
{
  "judgments": [
    {"id": "msg-123", "score": 0.78, "verdict": "relevant"},
    {"id": "msg-456", "score": 0.12, "verdict": "irrelevant"},
    {"id": "msg-789", "score": 0.45, "verdict": "investigate"}
  ]
}
```

### MCP Tool

Same interface exposed as `judge_relevance` MCP tool for use by other components.

---

## Performance Estimate

| Case | Latency | Comparison |
|------|---------|------------|
| Normal (5 msgs, all judged) | ~100ms (embed 6 texts + cosine) | Current 15-fetch: 100-300ms |
| Investigate (expanded fetch) | ~400-500ms (additional API + embed) | Rare, acceptable |

jina-v5-nano local ONNX inference is fast (~50-100ms for 6 texts). The normal case is **equivalent or faster** than the current 15-message blind fetch.

---

## Threshold Strategy

### Phase 1: Fixed thresholds (v0.5.0)

```
T_high = 0.6  (env: DISCORD_CONTEXT_RELEVANCE_HIGH)
T_low  = 0.3  (env: DISCORD_CONTEXT_RELEVANCE_LOW)
```

Configurable via environment variables. Chosen conservatively -- can be tuned based on production data.

### Phase 2: Dynamic thresholds (future)

Adopt CPersona's autocut pattern -- detect score distribution gaps to dynamically determine relevance boundaries. Requires sufficient data volume (5 candidates may not be enough; better suited for expanded fetch sets).

---

## tool.search: Unified Search Interface (Future Scope)

Alongside the context judgment feature, `tool.websearch` will be elevated to a **general-purpose search router** (`tool.search`):

```
tool.search
  |-- source="web"       -> Current SearXNG/Tavily/DuckDuckGo (unchanged)
  |-- source="context"   -> Discord conversation context (semantic search)
  |-- source="memory"    -> CPersona recall (proxy)
  |-- source="auto"      -> Intelligent routing based on query
```

### Context Source Backend

- Uses `tool.embedding`'s index/search with namespace `context:{channel_id}`
- Kernel indexes conversation pairs (user message + bot response) at callback respond time
- Subsequent triggers query this index semantically instead of blind-fetching

### Embedding Server Extension Required

Current schema: `(namespace, item_id, vector, created_at)`
Required: `+ text TEXT, + metadata JSON` columns, + FTS5 virtual table for hybrid search

### CPersona Integration

`source="memory"` proxies to CPersona's recall via HTTP. Enables agents to search across memory, context, and web through a single `search` tool.

---

## Implementation Plan

### v0.5.0 Scope (Minimum Viable)

1. **Embedding server**: Add `judge_relevance` HTTP endpoint + MCP tool
2. **Discord Bridge**: 
   - Reduce `DISCORD_CONTEXT_HISTORY_LIMIT` default to 5
   - Call `judge_relevance` before building callback payload
   - Implement investigate → expanded fetch → re-judge loop (1-shot)
3. **Config**: New env vars `DISCORD_CONTEXT_RELEVANCE_HIGH`, `DISCORD_CONTEXT_RELEVANCE_LOW`

### v0.5.x (Follow-up)

4. **tool.websearch → tool.search**: Rename + add `source` parameter routing
5. **Embedding server**: Add text + metadata storage, FTS5
6. **Kernel**: Index conversation pairs in tool.search at callback respond time
7. **CPersona**: Optionally delegate vector search to tool.search

### Open Questions

- **INVESTIGATE expansion strategy**: Fixed 20 messages, or adaptive based on INVESTIGATE count?
- **Dynamic thresholds**: When to introduce? After how much production data?
- **tool.search rename**: Breaking change for mcp.toml and kernel tool references. Coordinate timing.

---

## Related Changes

- **v0.4.2** (2026-04-03): Removed webhook/bot mention bypass to prevent runaway loops
- **CPersona v2.4.2**: Added `channel` column for context isolation
- **Embedding v0.2.0**: Added index/search with namespace isolation
