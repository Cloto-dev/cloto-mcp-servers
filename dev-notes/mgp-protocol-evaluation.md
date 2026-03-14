# MGP Protocol Evaluation — Multi-Protocol Comparative Analysis

> Internal reference. Date: 2026-03-06
> MGP Spec Version: 0.6.0-draft

---

## 1. Survey Scope

| Protocol | Author | Purpose | Status |
|----------|--------|---------|--------|
| **MCP** | Anthropic → AAIF/Linux Foundation | Agent→Tool connection | Stable (2025-11-25) |
| **A2A** | Google → LF AI & Data | Agent→Agent collaboration | Pre-1.0 (v0.3.0) |
| **ACP** | IBM | Agent→Agent (lightweight) | Merged into A2A (discontinued) |
| **OpenAI APIs** | OpenAI | Agent→Tool (proprietary) | Stable (Responses API) |
| **AGNTCY** | Cisco/LangChain → Linux Foundation | Agent infrastructure layer | Early-stage |
| **ANP** | OSS Community | Decentralized agent network | Research-stage |
| **MGP** | ClotoCore Project | MCP strict superset + security + tool intelligence | Draft 0.6.0 |

---

## 2. MGP Architecture Summary

### 2.1 Protocol Primitives (39 total)

| Category | Count | Layer |
|----------|-------|-------|
| Protocol Methods | 4 | Layer 3 (Methods) |
| Protocol Notifications | 8 | Layer 2 (Notifications) |
| Kernel Tools | 17 | Layer 4 (Tools) |
| Metadata Fields | 12+ | Layer 1 (Metadata) |

**4-Layer Architecture:**

```
Layer 4: Kernel Tools (17 — standard tools/call)
  mgp.access.* (query, grant, revoke)
  mgp.audit.* (replay)
  mgp.health.* (ping, status)
  mgp.lifecycle.* (shutdown)
  mgp.events.* (subscribe, unsubscribe, replay)
  mgp.discovery.* (list, register, deregister)
  mgp.tools.* (discover, request, session, session.evict)

Layer 3: Protocol Methods (4 — irreducible)
  mgp/permission/await, mgp/permission/grant
  mgp/callback/respond, mgp/stream/cancel

Layer 2: Protocol Notifications (6 — fire-and-forget)
  notifications/mgp.audit, notifications/mgp.lifecycle
  notifications/mgp.stream.chunk, notifications/mgp.stream.progress
  notifications/mgp.stream.pace, notifications/mgp.stream.gap
  notifications/mgp.callback.request, notifications/mgp.event

Layer 1: Metadata Extensions (0 new methods)
  _mgp.stream, _mgp.seq, _mgp.attempt, _mgp.delegation
  _mgp.source, _mgp.admin_only, effective_risk_level, risk_level
```

### 2.2 Security Architecture (5 layers)

| Layer | Mechanism | Spec Section |
|-------|-----------|-------------|
| Permission System | 11 standard permissions + scoped grants + deny-first | §3 |
| Access Control | Agent→Tool hierarchical RBAC | §5 |
| Audit Trail | 12 standard event types + trace_id + _mgp.seq + replay | §6 |
| Code Safety | 4-level validation (unrestricted/standard/strict/readonly) | §7 |
| OS Isolation | L0 Magic Seal, L1 resource limits, L2 FS, L3 network, L4 process | ISOLATION_DESIGN |

### 2.3 Reliability Mechanisms

| Mechanism | Direction | Recovery |
|-----------|-----------|----------|
| `_mgp.seq` + `mgp.audit.replay` | K→S | Sequence-based gap detection + catch-up |
| `_mgp.seq` + `mgp.events.replay` | K→S | Per-subscription replay (min 1000 buffer) |
| Callback retry + kernel dedup | S→K | Same callback_id, max 3 retries, 5s interval |
| Streaming gap detection | K→S | `mgp.stream.gap` + retransmission or final response fallback |

### 2.4 Tool Discovery (MGP's primary differentiator)

| Mode | Tool | Purpose |
|------|------|---------|
| Mode A | `mgp.tools.discover` | Keyword/semantic/category search at runtime |
| Mode B | `mgp.tools.request` | Agent-initiated tool acquisition + autonomous generation |

**Context Reduction:** 150,000 tokens (all tools) → 1,000-2,000 tokens (discovery + cache) = **99% reduction**

### 2.5 Extension System (12 independently negotiable)

| Tier | Extensions | Adoption Time |
|------|-----------|---------------|
| Tier 1 (Minimal) | `tool_security` | Hours |
| Tier 2 (Security) | + `permissions`, `access_control`, `audit`, `code_safety` | 1 week |
| Tier 3 (Communication) | + `lifecycle`, `streaming`, `progress`, `callbacks`, `events`, `discovery` | 2-4 weeks |
| Tier 4 (Intelligence) | + `tool_discovery` | 1-2 months |

---

## 3. Multi-Axis Comparison

### 3.1 Security Architecture

| Item | MCP | A2A | OpenAI | MGP |
|------|-----|-----|--------|-----|
| Authentication | OAuth 2.1 + PKCE | OAuth 2.0 / OIDC / mTLS | API keys | MCP inherited + trust_level |
| Permission model | None | None | None | 11 standard + scopes + deny-first |
| Access control | None | Skill-level auth metadata | None | Agent→Tool hierarchical RBAC |
| Audit trail | None | None | None | 12 standard event types + trace_id + replay |
| Code safety | None | None | Code Interpreter (proprietary sandbox) | 4-level validator (§7) |
| Trust levels | None | Agent Card verification | None | 4-level + kernel enforcement + Magic Seal |
| OS isolation | None | None | OpenAI-managed | L0-L4 (Seal/resources/FS/NW/process) |
| Known vulns | **Severe** (43% cmd injection, 22% path traversal) | Implementation-dependent | Low (proprietary infra) | Spec-level mitigation (untested) |

### 3.2 Protocol Design

| Item | MCP | A2A | OpenAI | MGP |
|------|-----|-----|--------|-----|
| Wire format | JSON-RPC 2.0 | JSON-RPC 2.0 / gRPC | REST JSON | JSON-RPC 2.0 (MCP-compatible) |
| Primitive count | Tools + Resources + Prompts + Sampling + Elicitation + Tasks | Tasks + Messages + Skills | Functions + Responses | 12 core (4 methods + 8 notifications) + 17 kernel tools + 12 metadata |
| Extension mechanism | Add servers | Agent Card | API updates | 12 independently negotiable extensions |
| Layer separation | None (flat) | None (flat) | None | 4-layer (Metadata/Notifications/Methods/Tools) |
| Transport | stdio, Streamable HTTP | HTTP(S), SSE, gRPC, Webhooks | HTTPS REST | MCP inherited + WebSocket designed |

### 3.3 Communication Patterns

| Pattern | MCP | A2A | OpenAI | MGP |
|---------|-----|-----|--------|-----|
| Request-Response | Full | Full | Full | Full |
| Streaming | Streamable HTTP | SSE + gRPC | Response streaming | Chunk notifications + flow control + gap detection |
| Callbacks | Sampling / Elicitation | input_required state | None | 5 types (confirmation/input/selection/notification/llm_completion) |
| Event subscription | None | Webhooks | None | subscribe/unsubscribe + _mgp.seq + replay |
| Async tasks | Tasks (2025-11) | Task lifecycle + webhooks | Responses API | None (achievable via §19 patterns) |
| Delegation | None | Agent-to-agent delegation | Agents SDK handoff | _mgp.delegation + permission intersection |

### 3.4 Tool Management

| Item | MCP | A2A | OpenAI | MGP |
|------|-----|-----|--------|-----|
| Tool discovery | Static (tools/list) | Agent Card skill ads | Defined in API request | Dynamic (keyword/semantic/category) |
| Context optimization | None (inject all) | N/A | None | Session cache + context budget (99% reduction) |
| Autonomous tool generation | None | None | None | Mode B + 6-tier safety guardrails |
| Tool output schema | Yes (2025-06) | N/A | strict mode | MCP inherited |

---

## 4. Scoring: 100-Point Multi-Axis Evaluation

### Axes and Weights

| Axis | Weight | Criteria |
|------|--------|----------|
| Security | 20 | Permissions, access control, audit, isolation, code safety |
| Protocol Design | 15 | Primitive design, layer separation, extensibility, minimalism |
| Communication | 10 | Bidirectional, streaming, callbacks, events |
| Reliability | 10 | Delivery guarantees, gap detection, replay, retry |
| Tool Management | 15 | Discovery, context optimization, session management |
| Interoperability | 10 | Ecosystem compatibility, migration path, standards compliance |
| Production Readiness | 10 | Implementation maturity, production deployments, battle-testing |
| Spec Quality | 5 | Completeness, clarity, versioning, documentation |
| Innovation | 5 | Structural breakthroughs, unique features |

### Scores

| Axis (Weight) | MCP | A2A | OpenAI | MGP | Rationale |
|---------------|-----|-----|--------|-----|-----------|
| Security (20) | 8 | 10 | 7 | **19** | MCP: OAuth 2.1 exists but no permissions/audit/isolation, severe implementation vulns. A2A: mTLS + signed Cards. OpenAI: proprietary infra but no protocol-level security. MGP: 5-layer security, -1 for unimplemented |
| Protocol Design (15) | 11 | 9 | 7 | **14** | MCP: Refined primitives (Tasks added completeness). A2A: dual JSON-RPC+gRPC. OpenAI: proprietary, non-extensible. MGP: 4-layer separation + selective minimalism (25→12 reduction), -1 for asymmetry |
| Communication (10) | 7 | 8 | 5 | **9** | MCP: Streamable HTTP + Tasks. A2A: SSE+gRPC+Webhooks. OpenAI: basic streaming only. MGP: 5-type callbacks + flow control + gap detection, -1 for asymmetry |
| Reliability (10) | 5 | 6 | 8 | **9** | MCP: no delivery guarantee mechanisms. A2A: task state machine + webhooks. OpenAI: high reliability via proprietary infra. MGP: _mgp.seq + replay + retry + dedup, -1 for JSON-RPC constraint |
| Tool Management (15) | 6 | 3 | 7 | **15** | MCP: static tools/list only. A2A: no tool concept (skills). OpenAI: strict mode + built-in tools. MGP: dynamic discovery + autonomous generation + 99% context reduction = full marks |
| Interoperability (10) | **10** | 7 | 4 | 8 | MCP: AAIF, 97M+ downloads, universal adoption. A2A: MCP complement, 150+ orgs. OpenAI: proprietary. MGP: strict MCP superset + staged migration path, -2 for no independent ecosystem |
| Production (10) | 7 | 2 | **10** | 2 | MCP: 10K+ servers, security issues. A2A: rare production deploys. OpenAI: millions of developers. MGP: spec-complete 30% implemented, no production deployment |
| Spec Quality (5) | 4 | 3 | 3 | **5** | MCP: good but distributed. A2A: evolving. OpenAI: API docs (no formal spec). MGP: 3,862 lines, version history, cross-reference validated |
| Innovation (5) | 3 | 3 | 2 | **5** | MCP: Tasks is sole innovation. A2A: Agent Cards. OpenAI: strict mode. MGP: dynamic discovery, autonomous generation, selective minimalism, OS isolation integration |
| **Total** | **61** | **51** | **53** | **86** | |

### Score Visualization

```
Security (20)       MCP ████░░░░░░░░░░░░░░░░  8
                    A2A █████░░░░░░░░░░░░░░░  10
                 OpenAI ███▌░░░░░░░░░░░░░░░░   7
                    MGP █████████▌░░░░░░░░░░  19  ★

Protocol (15)       MCP ███████▎░░░░░░░  11
                    A2A ██████░░░░░░░░░   9
                 OpenAI ████▋░░░░░░░░░░   7
                    MGP █████████▎░░░░░  14  ★

Communication (10)  MCP ███████░░░  7
                    A2A ████████░░  8
                 OpenAI █████░░░░░  5
                    MGP █████████░  9  ★

Reliability (10)    MCP █████░░░░░  5
                    A2A ██████░░░░  6
                 OpenAI ████████░░  8
                    MGP █████████░  9  ★

Tool Mgmt (15)      MCP ██████░░░░░░░░░  6
                    A2A ███░░░░░░░░░░░░  3
                 OpenAI ████▋░░░░░░░░░░  7
                    MGP ███████████████  15  ★

Interop (10)        MCP ██████████  10  ★
                    A2A ███████░░░  7
                 OpenAI ████░░░░░░  4
                    MGP ████████░░  8

Production (10)     MCP ███████░░░  7
                    A2A ██░░░░░░░░  2
                 OpenAI ██████████  10  ★
                    MGP ██░░░░░░░░  2

Spec Quality (5)    MCP ████░  4
                    A2A ███░░  3
                 OpenAI ███░░  3
                    MGP █████  5  ★

Innovation (5)      MCP ███░░  3
                    A2A ███░░  3
                 OpenAI ██░░░  2
                    MGP █████  5  ★
```

---

## 5. MGP Structural Advantages (Unique to MGP)

Features that exist in NO other surveyed protocol:

| Feature | Nearest Competitor | MGP's Delta |
|---------|-------------------|-------------|
| **Dynamic Tool Discovery** (§16 Mode A) | None | Keyword+semantic+category search at runtime |
| **Autonomous Tool Generation** (§16 Mode B) | None | Agent detects capability gap → auto-generates tool (6-tier safety) |
| **99% Context Reduction** | Cursor 40-tool cap (workaround) | Session cache + context budget: 150K→1-2K tokens |
| **OS Isolation Integration** | OpenAI Code Interpreter (proprietary) | Open spec, L0-L4, trust_level-linked, cross-platform |
| **effective_risk_level** | None | Kernel-derived authoritative risk, not self-declared |
| **Permission Scopes** (deny-first) | None | `paths`/`deny_paths` granularity, deny-first resolution |
| **Notification Reliability Compensation** | A2A Webhooks (partial) | `_mgp.seq` + replay tools + retry + dedup integrated system |
| **Selective Minimalism** | None | 25→12 primitives while preserving full functionality |
| **4-Layer Architecture** | None | Clean transport-dependent/independent separation |

---

## 6. MGP Weaknesses

| Weakness | Impact | Mitigation |
|----------|--------|------------|
| **Zero production track record** | Spec's practicality unverified | ClotoCore as reference implementation (in progress) |
| **No ecosystem** | No third-party servers/SDKs/tools | MCP strict superset = existing MCP ecosystem usable |
| **JSON-RPC asymmetry** | Server→Kernel method calls impossible | Compensation mechanisms cover practical cases (§20 for future eval) |
| **Async tasks undefined** | No MCP Tasks-equivalent primitive | Achievable via §19 application patterns (not protocol-level) |
| **~30% implemented** | Gap between spec and reality | Staged implementation (Tier 1→4) |

---

## 7. Strategic Positioning

```
                    Agent-to-Tool ◄──────────────────► Agent-to-Agent
                         │                                    │
  Proprietary    OpenAI ─┤                                    │
                         │                                    │
  Open Standard  MCP ────┤                              A2A ──┤
                         │                                    │
  MCP Superset   MGP ────┤                                    │
                         │                                    │
  Infrastructure         │                            AGNTCY ─┤
                         │                                    │
  Decentralized          │                              ANP ──┤
```

**MGP's position:** MCP's security & intelligence gap filler. Not competing with A2A (different axis). Strategic advantage via MCP backward compatibility — no cold-start problem.

---

## 8. MCP Ecosystem Context (March 2026)

Key facts for competitive awareness:

- **97M+ monthly SDK downloads** (Python + TypeScript)
- **10,000+ active MCP servers**, 5,800+ on PulseMCP registry
- **Donated to AAIF** (Linux Foundation, Dec 2025) — platinum members: AWS, Anthropic, Block, Bloomberg, Cloudflare, Google, Microsoft, OpenAI
- **MCP spec 2025-11-25** added: Tasks, Sampling with Tools, URL Mode Elicitation, Client ID Metadata Documents
- **Streamable HTTP** replaced SSE (2025-03-26)
- **OAuth 2.1** authorization (2025-06)
- **Security remains critical weakness**: CVE-2025-6514 (CVSS 9.6), 43% of servers have cmd injection, 22% have path traversal
- **ACP (IBM) merged into A2A** (2025-09)
- **A2A v0.3** added gRPC support, Agent Card signing, but still pre-1.0

---

## 9. Recommended Priorities

Based on this analysis, MGP's development should prioritize:

1. **Production readiness** (score: 2/10) — the single biggest gap
2. **MCP Tasks equivalence** — evaluate whether §19 patterns suffice or protocol primitive needed
3. **SDK development** (Python/TypeScript) — prerequisite for ecosystem growth
4. **Security validation** — MGP's top scoring axis must survive real-world testing
5. **Monitor MCP evolution** — AAIF governance may accelerate features that overlap with MGP

---

*Sources: MCP Specification (2025-11-25), A2A Specification (v0.3.0), OpenAI API docs, AGNTCY docs, ANP White Paper, MGP_SPEC.md (0.6.0-draft), MGP_ISOLATION_DESIGN.md (0.1.0-draft)*
