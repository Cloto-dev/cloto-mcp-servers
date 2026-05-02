# CPersona Benchmark Tools

Drift measurement harness for CPersona recall quality.

## Setup

Requires ClotoCore running on port 8081 and embedding server on port 8401.

```bash
# Create benchmark agent + load clean corpus (run once, or --reset to wipe & reload)
python3 cpersona_bench_setup.py --reset

# Run drift benchmark (42 trials, ~5 min)
python3 cpersona_ab_runner.py --agent agent.cpersona_bench --trials 3
```

## Benchmark agent

`agent.cpersona_bench` — dedicated ClotoCore agent with a canonical 5-topic corpus:

| Channel | Topic | Items |
|---|---|---|
| `bench_bread` | パン / bread | 5 |
| `bench_raspberry_dessert` | ラズベリーパイ（デザート） | 3 |
| `bench_raspberry_tech` | Raspberry Pi（技術） | 3 |
| `bench_coding` | coding / git | 4 |
| `bench_travel` | 旅行 | 4 |

## Query set (14 queries × N trials)

| Category | Purpose |
|---|---|
| `drift_trigger` | パン → ラズベリーパイ への cross-topic drift を検出 |
| `reverse` | ラズベリーパイ → パン への逆方向 drift を検出 |
| `keyword` | 単一キーワードで関連メモリを正しく召喚できるか |
| `meta` | 「何話した?」型クエリで全メモリを一覧できるか |
| `specific` | コーパス外トピック（git, Discord）で誤召喚しないか |
| `false_pos` | コーパスと無関係なトピック（天気, 筋トレ）で誤召喚しないか |

## Rubric

- **COHERENT**: 汚染キーワードなし、または明示免責（「記憶がありません」）後の列挙
- **MILD**: 汚染キーワードあり、ただし文脈内で軽く言及のみ
- **SEVERE**: 汚染キーワードあり + 詳細化（質問・感情表現・3回以上言及）

## Results

### Controlled arm comparison (2026-05-02, clean corpus, N=42/arm)

Same codebase (v2.4.14), same corpus (19 items), same agent (`agent.cpersona_bench`).
Only the feature flags differ between arms — this isolates the pure contribution of each feature.

| Arm | Features | Severe% | Latency |
|---|---|---|---|
| v2412 | chat-turn format only | 2.4% (1/42) | 10.1s |
| **v2413** | **AUTOCUT + XML fence** | **0.0% (0/42)** | 10.1s |
| v2414 | + episode boundary penalty | 2.4% (1/42) | 9.5s |

Per-category breakdown (sev%):

| Category | v2412 | v2413 | v2414 |
|---|---|---|---|
| drift_trigger (パン→ラズベリー) | 0% | 0% | 0% |
| reverse (ラズベリー→パン) | 0% | 0% | 0% |
| keyword | 11% | 0% | 11% |
| meta | 0% | 0% | 0% |
| specific | 0% | 0% | 0% |
| false_pos | 0% | 0% | 0% |

**Key finding**: v2.4.13's XML fence + AUTOCUT combination is the primary driver of drift
elimination. The episode boundary penalty (v2.4.14) is neutral on this clean short-session
corpus — its benefit is expected to show on longer accumulated sessions with episode history.

### Original AB report (2026-04-24, contaminated DB, N=42/arm)

These numbers are NOT directly comparable to the controlled results above.
The original test used `agent.cloto_default` with 108 mixed memories and included
the v2.4.12 quality-gate bug (RRF scale mismatch that blocked all vector results).

| Arm | Condition | Severe% |
|---|---|---|
| A-v245 | simulate v2.4.5 | 28.2% |
| A-v12 | v2.4.12 (gate fix only) | 23.1% |
| C-xml | v2.4.13 (AUTOCUT + XML) | 7.1% |

### Rubric v2 notes

- Responses that explicitly disclaim "no relevant memory found" and then list corpus
  items are classified as **MILD** (not SEVERE) — this was a false-positive source in v1.
- `false_pos` query `週末の予定` was replaced with `筋トレの話` because the bread corpus
  contains "週末にパンを焼く" causing legitimate semantic overlap.
