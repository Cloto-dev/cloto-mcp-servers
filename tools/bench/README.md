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

## Historical results

| Version | Condition | Severe% |
|---|---|---|
| A-v12 (v2.4.12) | baseline | 23.1% |
| C-xml (v2.4.13) | AUTOCUT + XML fence | 7.1% |
| v2.4.14 | per-agent threshold | TBD (benchmark corpus v2) |
