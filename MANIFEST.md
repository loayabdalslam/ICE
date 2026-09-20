# ICE complete export — 2026-09-15

## What this tree is

Agent harness named **ICE**: Intent → Compile → Execute → Verify → Δ.
Rust TUI + Python Harbor adapter + harness-only bench + sample web app.

## Commands

```
ice                         TUI, writes .ice/ in cwd
ice init [DIR]              full workspace on any path
ice -p "<goal>" [--demo] [--goal-loop] [--max-turns N]
ice inspect | ice version
python3 harness_bench/run_harness_bench.py
python3 examples/test_skills_mcp.py
```

## Shipped

| Path | Role |
| --- | --- |
| src/ | Rust engine + TUI (theme ice default) |
| src/init.rs | `.ice/` bootstrap |
| .ice/ | settings, config, providers, mcp, skills, todos, goal |
| harbor_adapter/ | Harbor BaseAgent + engine + metrics |
| harbor_bench/ | local Harbor-format tasks + Groq JSON |
| harness_bench/ | **no-LLM** infrastructure bench + results |
| skills/ | ice-burst SKILL.md |
| examples/ | MCP echo + protocol tests |
| webapp/ | ICE Freeze task board |
| demo/ | CLI + webapp videos |
| README.md | harness comparison vs other CLIs |
| HARNESS_BENCH.md | raw ICE microbench |
| EXPORT.md | system map |

## Not in the zip

`target/`, frame PNG dumps, `__pycache__`, Harbor work copies, `run.pid`.

## Honest limits

- Rust binary was not produced in this sandbox (crate registry).
- Harbor TB2.0 full Docker suite was not run.
- Other-CLI numbers in README are published 2026 papers/benches, not re-runs on this host.
- ICE microbench **is** measured here (harness_only.json).
