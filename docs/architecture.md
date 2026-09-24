# ICE architecture

ICE is a single Rust binary. The interactive REPL, the agent engine and the tools all run in-process; the only external processes are the commands ICE runs for you, MCP servers, hooks, and (optionally) a CLI backend.

## Module map

| Layer | Module | Responsibility |
| --- | --- | --- |
| Entry | `main.rs` | clap CLI: flags, subcommands (`login`, `mcp`, `config`, `update`, `doctor`, …), print vs interactive |
| UI | `repl.rs` | Inline REPL: event loop, transcript items, live region, keys, engine worker thread |
| | `commands.rs` | Slash commands, pickers (onboarding, model, resume, rewind, config), permission dialog |
| | `render.rs` | Styled lines, word-wrap, markdown (tables, code, lists), tool-call and diff views |
| | `term.rs` | Ink-style renderer: commit finished lines to scrollback, redraw only the live region |
| | `editor.rs` | Multi-line prompt editor with readline keys and soft-wrap caret math |
| | `theme.rs`, `mascot.rs` | Colour themes (truecolor → 256 fallback), FLOE and the ICE wordmark |
| Engine | `engine.rs` | The agent loop: stream → permission → tools (parallel batches) → results → repeat; interrupts, compaction, sub-agents, hooks, stats |
| | `model.rs` | Provider adapters: Anthropic Messages (caching, thinking), OpenAI Chat Completions (tools, reasoning), text tool protocol fallback, pricing |
| | `conv.rs` | Provider-neutral messages (`text`, `thinking`, `tool_use`, `tool_result`) and usage |
| | `prompt.rs` | System prompt, `<env>` and git snapshot, memory files with `@imports`, thinking budgets |
| Tools | `tools.rs` | Read/Write/Edit/MultiEdit/Glob/Grep/LS/Bash/BashOutput/KillShell/TodoWrite/WebSearch/Skill, and web fetch helpers |
| | `mcp.rs` | MCP client (stdio + streamable HTTP), server pool, config files |
| | `agents.rs`, `skills.rs` | Built-in and custom sub-agents; skills |
| Safety | `permissions.rs` | Modes, rule matching, safe-command detection, rule suggestions |
| | `sandbox.rs` | Symlink-aware path containment, last-resort command deny-list |
| | `hooks.rs` | Lifecycle hooks with block/allow semantics |
| State | `settings.rs` | Layered settings, credentials (0600) |
| | `store.rs` | JSONL transcripts, resume, history repair, cleanup |
| | `oauth.rs` | OAuth 2.0 + PKCE browser sign-in, token refresh |
| | `providers.rs` | Provider catalogue, CLI backends, model discovery, active config |
| Infra | `http.rs` | In-process HTTPS (ureq/rustls): retries with backoff, SSE, proxy + NO_PROXY |
| | `update.rs` | Source (git + cargo) and binary (SHA-256 verified) updates |

## Life of one turn

1. **Submit.** The REPL sends `Cmd::Submit` to the engine worker. `UserPromptSubmit` hooks run. `@file` mentions are attached as simulated `Read` results, and plan mode adds a reminder.
2. **Request.** The engine builds the system prompt (instructions, `<env>`, git status, memory, goal) and the tool list (built-ins, skills, agents, MCP), then streams a turn from the adapter. Text and thinking deltas reach the UI live. Transient errors (429/5xx/network) are retried with jittered backoff.
3. **Tools.** Each `tool_use` block is validated against its schema and passed through `PreToolUse` hooks and the permission checker. Allowed read-only tools accumulate into a batch that runs in parallel threads. Anything that needs a prompt, or has side effects, runs in order after the batch is flushed. Asking the user blocks on a channel the REPL answers from the dialog.
4. **Results.** Outputs become `tool_result` blocks, and `PostToolUse` hooks run. Diffs, todo lists and command output are rendered for the transcript.
5. **Repeat** until the model answers without tools. `Stop` hooks can send it back to work, and max-turns bounds the loop. Before any request that would overflow the window, the conversation is compacted into a summary.
6. **Persist.** Every message and usage record is appended to the session transcript, which `--continue`, `--resume` and `/rewind` rebuild from.

Interrupting sets a shared flag. It aborts the stream between chunks, kills a running command's whole process group, and records `[Request interrupted by user]` so the conversation stays valid for the API.
