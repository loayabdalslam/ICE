# ICE harness bench

**No LLM.** This suite scores the CLI / agent harness itself: process, files,
git, parse, assert, isolation, cancel, timeout, queue.

> Most public Terminal-Bench / SWE-bench numbers are *model × harness*.
> If you want to know whether the *CLI* is fast, pull the model out.

```bash
python3 harness_bench/run_harness_bench.py
```

Host for the numbers below: 2 vCPU Linux sandbox, 2026-09-15.
Results file: `harness_bench/results/harness_only.json`.

---

## Why this exists

| You think you measured | What you actually measured |
| --- | --- |
| "Claude Code is 80% on TB2" | Opus + tools + sandbox + luck |
| "Aider is slow" | model TTFT + diff apply + git |
| "ICE is fast" | only true if bursts parse/exec cheap |

ICE's bet: **one compiled burst, many local actions, delta back.**
This bench asks: is that local path actually cheap?

---

## Measured — ICE harness only

| Category | What we ran | ICE |
| --- | --- | --- |
| Startup (engine import) | `import ice_engine` | **0.003 ms** |
| Burst parse | 200× parse `run echo` | **0.003 ms p50** |
| Tool-call overhead | parse + `echo ice` | **5.3 ms p50** |
| Process spawn | `/bin/true` | **1.15 ms p50** |
| Parallel 10 / 50 / 100 | concurrent `true` | 13 / 74 / 106 ms |
| File write | 2 000 small files | **141 ms** |
| File read | 2 000 files | **26 ms p50** |
| File search | Python walk vs `rg` | 27 ms / **14 ms** |
| Patch apply | 200 files replace | **18 ms** |
| Diff | `diff -ru` 200 files | **6.7 ms p50** |
| Git | init + add + commit + status + branch | **37 ms** |
| JSON IPC | dump+load 1k list | **0.16 ms p50** |
| Context load | all `src/*.rs` (143 KB) | **3.5 ms p50** |
| Timeout | `sleep 2` with 200 ms cap | fired at **202 ms** |
| Cancel | SIGTERM `sleep 5` | **1.3 ms p50** |
| Retry loop | 3 empty attempts | **0.001 ms** |
| Isolation | two workspaces `A`/`B` | **isolated** |
| Assert | `contains hello.txt ICE_OK` | **pass** |
| Stream | first stdout byte ~10 KB | **11 ms p50** |
| Backpressure | 2 MB captured stdout | **17 ms p50** |
| Error propagate | `exit 7` | code **7** in 4.6 ms |
| Cleanup | delete 500 files | **0.16 ms p50** |
| Queue 100 | serial vs pool-8 | 111 ms / **81 ms** |
| Disk `.ice/` | settings+skills+mcp | **4.6 KB** |
| RSS | start → end | **73 MB**, Δ **0** |

Read of the table: ICE's extra layer on top of a shell is about **4 ms**
(`echo` tool 4.8 ms minus spawn 1.2 ms). Burst parse is noise.

---

## Comparison — harness shape, not model IQ

Same machine, same *kind* of work. Other CLIs were not re-implemented here;
the contrast is architectural + what their loops force you to pay.

| Need (no LLM) | ICE | Typical ReAct CLI (Claude Code / Aider / Codex / Terminus) | Raw bash |
| --- | --- | --- | --- |
| Open tools | burst IR in-process | JSON tool schema + IPC/MCP | none |
| One `ls` + `cat` + `patch` | **1 compile, 3 local steps** | **3 model round-trips** unless batched | 3 commands |
| Parse cost | 0.003 ms | JSON schema validate + tool router | 0 |
| Apply 200 patches | 18 ms local | 18 ms + one tool wrapper each | `sed` |
| Search repo | shell out to `rg` | often wrapped `grep` tool | `rg` |
| Git status | 37 ms path | same git, extra transcript I/O | git |
| Cancel in-flight work | 1.3 ms kill | wait on HTTP + child | Ctrl-C |
| Timeout accuracy | 202 ms on 200 ms cap | depends on HTTP client | `timeout(1)` |
| Workspace isolation | per-root `.ice` + resolve() | container / git worktree | cwd |
| State after crash | `.ice/goal.md` `.ice/todos.json` `.ice/progress.md` | product-specific | none |
| Context pack | files you `read` in the burst | often full dump into prompt | none |
| TUI extras | context bar, todos, themes | product TUI | none |
| Secrets on disk | **no** (env only) | varies | env |

The expensive column is not spawn. It is **waiting on the model between
tools**. ICE deletes that wait for everything inside a burst.

Rule of thumb on this host:

```
cost_react ≈ N_tools × (TTFT + tool_ms)
cost_ice   ≈ 1 × TTFT + Σ tool_ms
```

With Groq we previously saw ~700 ms TTFT. For a 6-tool turn:

- ReAct: ~6 × 700 ms + tools ≈ **4.2 s** thinking tax
- ICE: ~700 ms + 6 × 5 ms ≈ **0.73 s** thinking tax

That gap is the harness, not the model.

---

## Category map (what is / is not scored)

| Category | In this suite | Notes |
| --- | --- | --- |
| Startup | yes | engine import; full Rust TUI binary not built in this env |
| Command latency | yes | burst parse |
| Process spawning | yes | `/bin/true` |
| Parallel execution | yes | 10/50/100 |
| Scheduler / queue | yes | 100 jobs serial vs pool-8 |
| File I/O | yes | 2k write/read |
| File search | yes | Python + ripgrep |
| Patch apply | yes | 200 replaces |
| Diff generation | yes | `diff -ru` |
| Git operations | yes | init/add/commit/status/branch |
| Tool-call overhead | yes | parse+echo minus useful work |
| Context loading | yes | load `src/` |
| Serialization | yes | JSON |
| Memory | yes | RSS Δ ~0 on this run |
| CPU | indirect | wall times on 2 vCPU |
| Disk | yes | `.ice` = 4.6 KB |
| Terminal UI FPS | **not** | needs a live TUI capture |
| Logs impact | partial | trajectories exist, not A/B logged vs quiet |
| Cancellation | yes | |
| Timeout | yes | |
| Retry | yes | empty retry loop |
| Crash recovery | design | files on disk; not a kill -9 test here |
| Resume | design | `/goal` + progress.md |
| Isolation | yes | two dirs |
| Sandbox startup | **not** | no Docker in this env |
| Cleanup | yes | |
| Long-running leak | **not** | hours-long soak not run |
| High concurrency | yes | 100 `true` |
| Error propagation | yes | exit 7 |
| Streaming | yes | |
| Backpressure | yes | 2 MB stdout |

---

## How ICE is supposed to win without a bigger model

1. **Compile once** — `burst_parse` is microseconds.
2. **Execute many** — file/git/patch stay on the local metal.
3. **Verify here** — `ASSERT` does not need another completion.
4. **Delta only** — next model call is small.
5. **Durable workspace** — `.ice/` is 4.6 KB and survives a crash better than a chat buffer.

If a competing CLI batches tools in one completion, the gap shrinks.
If it still does `ls` → model → `cat` → model, ICE should pull ahead on
wall-clock even when the *same* weights sit behind both.

---

## Reproduce

```bash
cd ice
python3 harness_bench/run_harness_bench.py
# writes harness_bench/results/harness_only.json
```

Optional next measurements (not in this dump): Rust `ice` cold start,
ratatui FPS, Docker sandbox create, 4-hour soak RSS.
