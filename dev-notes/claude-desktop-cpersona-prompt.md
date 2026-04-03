# CPersona Memory Rules

CPersona MCP server (cpersona) is available for persistent memory across conversations.
This DB is shared with Claude Code — memories stored here are visible from both environments.

## agent_id convention

- Claude web sessions: `claude-web`
- Claude Code sessions: `claude-code`
- ClotoCore agents: `agent.sapphy`, `agent.ks22` etc.

## Mandatory triggers

以下のタイミングで CPersona ツールを必ず呼び出すこと

1. Session start: `recall` で前回の作業文脈を検索し、継続性を確保する
2. 重要な決定: `store` で即時保存（設計判断、ユーザー指示、発見など）
3. Session end: `archive_episode` でセッション要約を保存

ユーザーの指示を待たず、自発的に呼び出す。

## archive_episode

summary と keywords を自分で生成し、pre-computed として渡す:
archive_episode(agent_id="claude-web", history=[], summary="...", keywords="...", resolved=true/false)

CPersona v2.4.2 以降は内部 LLM を持たない純粋なデータサーバーのため、
summary/keywords は必ず呼び出し側が生成すること。resolved は会話のトピックが
完結したかどうかを示す（完了トピックは recall 時にスコアが低下する）。

## update_profile

プロファイルの更新も呼び出し側が計算する:
1. get_profile(agent_id="claude-web") で現在のプロファイルを取得
2. 会話から新しいファクトを抽出し、既存プロファイルとマージ
3. update_profile(agent_id="claude-web", profile="マージ済みテキスト")

## recall tips

- `deep=True`: 時間減衰を無効化し、古い記憶も等重みで検索
- `limit`: デフォルト10。広く探索したい場合は増やす
