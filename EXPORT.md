# ICE system export

**Intent → Compile → Execute → Verify → Δ**

Harness for an agentic CLI. The model compiles a burst; the local executor
runs it at machine speed and returns only a delta.

## Layout

```
ice/
  src/                 Rust TUI + engine
  harbor_adapter/      Harbor BaseAgent + metrics
  harbor_bench/        Harbor-format local suite + Groq results
  skills/              bundled SKILL.md packs
  examples/            MCP echo server + protocol tests
  webapp/              ICE Freeze sample app
  demo/                CLI task videos
  .ice/                workspace init tree (settings, mcp, skills, todos)
```

## CLI

```
ice                 TUI (writes .ice/ on launch)
ice init [DIR]      full workspace on any path
ice -p "<goal>"     headless burst
ice inspect
```

Slash: `/onboard /init /providers /models /clis /skills /mcp /todo /theme /think /goal /loop /agent`

Default theme: **ice**.

## .ice on every init

Created at `<path>/.ice/` without overwriting user files:

- settings.json — harness, UI, context, sandbox, paths
- config.json — provider/model (no secrets)
- providers.json — endpoint catalog
- mcp.json + mcp/echo.py
- skills/ice-burst/SKILL.md
- todos.json, goal.md, progress.md
- agents/, logs/, sessions/, cache/, exports/

## Harbor

```
harbor run -d terminal-bench@2.0 \
  -a harbor_adapter.ice_agent:IceHarborAgent \
  -m groq/qwen/qwen3.8-27b
```

Local suite recorded pass/fail, reward, setup/agent/verify ms, tokens, cost, errors, trajectory.

## Env (never stored in .ice)

`GROQ_API_KEY` `OPENAI_API_KEY` `XAI_API_KEY` `ANTHROPIC_API_KEY` `ICE_API_KEY`
`ICE_PROVIDER` `ICE_MODEL` `ICE_BASE_URL`
