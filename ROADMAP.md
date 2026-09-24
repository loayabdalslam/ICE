# ICE Roadmap

## Status after 0.5.0

Most of the plan below shipped in 0.5.0:

| Area | Status |
|---|---|
| Keys out of argv, no shared temp files, in-process HTTPS with retries, proxy + NO_PROXY | ✅ done |
| Permission modes, allow/deny/ask rules, symlink-aware containment, hooks | ✅ done |
| Streaming, native tool calling (Anthropic + OpenAI-compatible), text-protocol fallback | ✅ done |
| Claude-Code-compatible tools, background shells, sub-agents, MCP stdio + HTTP | ✅ done |
| clap CLI, print mode (text/json/stream-json), exit codes, completions, `ice doctor` | ✅ done |
| Layered settings, credentials store, OAuth 2.0 + PKCE browser sign-in | ✅ done |
| Sessions (`--continue`, `--resume`, `/rewind`), auto-compaction, `/context`, `/cost` | ✅ done |
| Inline REPL with the ICE home screen, markdown, diffs, pickers, panic-safe terminal | ✅ done |
| CI on 3 OSes, end-to-end tests against a mock LLM, clippy `-D warnings` | ✅ done |
| Installers that clone + build from source, `ice update` for source/binary installs | ✅ done |
| LICENSE, rewritten README, architecture doc | ✅ done |
| Binaries moved out of git to GitHub Releases, signed releases, package managers | ⏳ next |
| Syntax highlighting in diffs and code blocks, vim mode, image input | ⏳ next |
| End-to-end task benchmark vs other CLIs (graduation evaluation chapter) | ⏳ next |

The original plan follows for reference.

---

# Original plan (2026-09-24): from prototype to a finished CLI

Written 2026-09-24 against `0.4.4` (commit `121c619`).

## Where the project stands

**What works:**
- About 6.9k lines of Rust in 21 modules. It builds cleanly and all 17 unit tests pass.
- A real TUI (ratatui), a headless `-p` mode, `init`, `inspect` and self-`update`.
- 12 provider adapters, plus a CLI-agent backend that runs Codex, Claude Code, opencode and others.
- An agent loop with tools: read, write, replace, grep, shell, web and todo, plus MCP, skills, sub-agents and goals.
- A release workflow for 5 targets, with SHA-256 checked installers and a strong visual identity (FLOE, themes).

**What is holding it back:**

| Area | Finding | Where |
|---|---|---|
| Security | The API key is passed to `curl` as a command-line argument (`-H "Authorization: Bearer …"`), so any local user can read it with `ps`. | `src/llm.rs:163` |
| Security | Every request body goes to one shared file, `/tmp/ice-req.json`. Two sessions race on it, and prompts sit on disk in plain text. | `src/llm.rs:164` |
| Security | The "sandbox" is a substring deny-list (`rm -rf /`, `sudo`…). It is trivial to bypass (`rm -r -f /`, `bash -c`, env vars). Shell and write actions have no approval prompt. | `src/sandbox.rs:36` |
| Robustness | Every HTTP call goes through an external `curl`. There is no streaming, no retry or backoff, no timeout policy and no proxy/TLS configuration. | `llm.rs`, `providers.rs`, `update.rs`, `exec.rs` |
| Robustness | The Anthropic, Gemini and OpenAI-compatible paths are only partly separated. Native tool calling exists only for OpenAI-style `tool_calls`. | `src/llm.rs` |
| CLI UX | Arguments are parsed by hand. `--prompt` without a value silently becomes `None`, and there are no `--model`, `--provider`, `--output json`, `--yes` or `--quiet` flags, no shell completions and no man page. | `src/main.rs:52` |
| State | Conversations are held in memory only and capped at 40 turns. They are never saved, so they can't be resumed or exported. | `src/app.rs:85,948` |
| Config | Keys are only read from environment variables. There is no config file hierarchy (global → project → flags) and no keychain. | `src/providers.rs` |
| Quality | 17 tests, all unit tests. There are no integration tests, no mock LLM, no CI on PRs (only a release workflow), and 18 clippy warnings, including dead code such as `verify::all_ok`. | `.github/workflows/` |
| Repo hygiene | About 40 MB of binaries, videos and PNGs are committed (`releases/`, `demo/`, `brand/`). `LICENSE`, `licenses/`, `docs/` and `windows/` are linked from the README but missing. `MANIFEST.md` mentions `harbor_adapter/`, which doesn't exist. | repo root |
| Docs | The README is written for a binary-only repo ("source not included"), which contradicts this source repo. The benchmark table compares a no-LLM microbench with other tools' end-to-end numbers. | `README.md` |

---

## Phase 0: Fix the security problems (≈1 week, do first)

1. **Keep secrets out of argv.** Send headers through `curl --config -` on stdin, or better, move to an in-process HTTP client (Phase 1).
2. **Give each request its own temp file with 0600 permissions,** or stream the body over stdin, and delete it afterwards. Add a unit test for concurrent requests.
3. **Add a permission model** with three modes: `ask` (default), `auto-edit` and `yolo`. Every `shell`, `write_file`, `replace` and `web_fetch` call shows a diff or command preview with Allow once / Always / Deny. Store the rules in `.ice/settings.json` (`allow: ["Bash(cargo test*)"]`).
4. **Real isolation, opt-in:**
   - Linux: `bwrap` or Landlock
   - macOS: `sandbox-exec`
   - Windows: job objects
   - Fallback: run commands in a git worktree

   Keep the deny-list only as defence in depth, and document its limits.
5. **Redact secrets** in logs, the transcript and `inspect` output. Add a regex test.
6. Add `SECURITY.md` with a threat model and a disclosure address.

## Phase 1: Core engine hardening (≈2–3 weeks)

1. **Replace `curl` shell-outs** with `ureq` (sync, small, rustls) or `reqwest`, and put all network calls behind one `http` module that handles timeouts, retry with jittered backoff on 429/5xx, `HTTPS_PROXY` and custom CA bundles.
2. **Stream tokens** (SSE) for OpenAI-compatible, Anthropic and Gemini, show them live in the TUI, and support Ctrl+C mid-stream.
3. **Provider trait.** Give OpenAI-compatible, Anthropic Messages, Gemini, Ollama and CLI-agent each their own implementation of `Provider { stream(), list_models(), supports_tools() }`, using native tool/function calling on each. Fall back to the text IR (`ir.rs`) only when a model can't call tools.
4. **Context management:** estimate tokens per model, summarise older turns automatically, add `/compact`, and cap tool output with a "truncated, use read_file with offset" hint.
5. **Better tools:**
   - `edit` that applies multiple hunks, with a readable error when the old string isn't unique
   - `glob` and `ls`, and `grep` that uses ripgrep when installed and respects `.gitignore`
   - `read_file` with offset and limit
   - Background shell jobs with timeouts and kill
6. **Mature MCP support:** stdio and streamable HTTP transports, `resources` and `prompts` as well as `tools`, a per-server timeout, and restart when a server crashes.
7. **Cost and token accounting** for each turn and session, shown in the status bar and in `--output json`.

## Phase 2: A proper CLI surface (≈1–2 weeks)

1. **Move argument parsing to `clap` (derive),** with subcommands:
   ```
   ice [PROMPT]                       interactive; optional first message
   ice run -p "<goal>" [--max-turns N] [--goal-loop] [--output text|json|stream-json]
   ice init [DIR]      ice inspect      ice update [--check]
   ice config get|set|list|edit       ice models [--provider P]
   ice mcp add|list|remove            ice skills list|new
   ice sessions list|show|resume|export|rm
   ice completions bash|zsh|fish|powershell
   ice doctor                          # checks curl/bash/git/keys/terminal size
   ```
   Global flags: `--cwd`, `--provider`, `--model`, `--yes/--permission-mode`, `-q/--quiet`, `-v/--verbose`, `--no-color`, `--config FILE`.
2. **Proper exit codes** (0 ok, 1 task failed, 2 usage, 3 auth, 4 network, 130 interrupted). Honour `NO_COLOR`, and detect when stdout isn't a TTY so the tool works in pipes: `cat err.log | ice -p "explain"`.
3. **Layered config:**
   - `~/.config/ice/config.toml` for global settings
   - `.ice/config.toml` for the project
   - environment variables, then command-line flags, taking precedence in that order
   - Keys stored in the OS keychain (`keyring` crate), opt-in
4. **Sessions:** save each session as `.ice/sessions/<id>.jsonl`. Add `ice --continue` and `--resume <id>`, `/export md`, and `/rewind` (with git-stash checkpoints before each edit burst).
5. Project instructions file: automatically load `ICE.md` (and `AGENTS.md`/`CLAUDE.md` for compatibility) into the system prompt.

## Phase 3: TUI polish (≈1–2 weeks)

- Split `app.rs` and `ui.rs` (1.1k lines each) into components: composer, transcript, diff viewer, permission dialog, status bar.
- Multi-line composer (Shift+Enter), input history (↑), `@file` mentions with fuzzy completion, `/` command autocomplete, paste of large blocks.
- Diffs with syntax highlighting (`syntect`) and a collapsible tool-call view.
- Mouse scrolling, resize handling, a terminal-size check, and a restored cursor/screen after a panic (panic hook).
- Accessibility: `ICE_REDUCED_MOTION` already exists. Add a high-contrast theme and a plain `--no-tui` fallback.

## Phase 4: Testing & CI (runs in parallel, ongoing)

1. **CI on every PR** (`.github/workflows/ci.yml`): `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test` on Linux, macOS and Windows, plus `cargo deny` (licences and advisories).
2. **Mock LLM server** (a small in-repo HTTP fixture) so the agent loop, tool calls, retries and streaming are tested without keys.
3. **Integration tests** with `assert_cmd` and `insta` snapshots for CLI output and TUI frames (ratatui `TestBackend`, already partly used).
4. **Property and fuzz tests** for `ir.rs` parsing and `sandbox::resolve` path escapes (`cargo fuzz`).
5. Coverage with `cargo llvm-cov`. Aim for ≥70 % on `ir`, `exec`, `sandbox`, `llm` and `harness`.
6. Keep `harness_bench` as a CI job that reports a regression when a metric gets more than 20 % slower.

## Phase 5: Distribution & repository hygiene (≈1 week)

1. **Stop committing binaries.** Publish them as GitHub Release assets. Point the installers and `update.rs` at `releases/download/vX/…`. Move `demo/` and `brand/` sources to Git LFS or an external host, and purge the history if needed.
2. Add `LICENSE` (Apache-2.0) and generate `licenses/THIRD_PARTY.md` (`cargo about`). Create the `docs/` folder the README already links to, or fix the links.
3. Sign releases: Sigstore/cosign or minisign for the checksum files, Apple notarisation, and Windows Authenticode if a certificate is available.
4. Package managers: `cargo install ice-cli`, Homebrew tap, Scoop/winget, AUR, `.deb`/`.rpm` via `cargo-dist`.
5. `CHANGELOG.md` (Keep a Changelog), SemVer, and `release-please` or `cargo-release` automation.

## Phase 6: Documentation & graduation-project deliverables

For a graduation or grade project, the evaluators usually expect these besides the code:

| Deliverable | Content |
|---|---|
| `README.md` rewrite | A pitch, a 30-second quick-start, a GIF, feature list, architecture diagram, and an honest benchmark section (label which numbers are ICE micro-benchmarks and which are other tools' published end-to-end numbers). |
| `docs/architecture.md` | The Intent → Compile (IR) → Execute → Verify → Δ pipeline, a module map, a sequence diagram of one agent turn, and the threading model. |
| `docs/user-guide.md` | Every command and flag, configuration, providers, MCP, skills, permissions, sessions. |
| `docs/security.md` | Threat model, permission modes, sandbox guarantees and limits. |
| `CONTRIBUTING.md` + `CODE_OF_CONDUCT.md` + issue/PR templates | So the project reads like maintained open source. |
| Rustdoc | `#![warn(missing_docs)]` on public items, with `cargo doc` published to GitHub Pages. |
| Evaluation chapter | **An end-to-end task benchmark:** 20–30 small repo tasks such as "fix the failing test" or "add a flag", scored by success rate, turns, tokens, cost and wall time, run for ICE with 2–3 models and compared with at least one other CLI on the *same* tasks. This is the evidence evaluators look for; it complements the existing no-LLM microbench. |
| Report / thesis outline | Problem → related work (Claude Code, Codex CLI, Aider, OpenCode, Goose) → design → implementation → evaluation → limitations → future work. |
| Demo | A 3–5 minute recorded walkthrough (asciinema + video) and a live-demo script that uses `--demo` mode as an offline fallback. |

---

## Suggested order & milestones

| Milestone | Scope | Exit criteria |
|---|---|---|
| **M1 · Safe** (0.5.0) | Phase 0 + CI part of Phase 4 | No secrets in argv or shared temp files; permission prompts on by default; CI green with clippy `-D warnings` |
| **M2 · Solid** (0.6.0) | Phase 1 + mock-LLM tests | In-process HTTP, streaming, provider trait, native tools for 3 API families, ≥60 % coverage on the core |
| **M3 · Complete CLI** (0.7.0) | Phase 2 + Phase 3 | clap subcommands, config file, saved and resumable sessions, `ice doctor`, completions, multi-line composer, diff viewer |
| **M4 · Shippable** (1.0.0) | Phase 5 + Phase 6 | Binaries out of git, signed releases, package managers, full docs, E2E benchmark results, report and demo |

## Quick wins (a day or less each)

- [ ] Pass the auth header through stdin or a 0600 config file instead of argv (`src/llm.rs`, `src/providers.rs`)
- [ ] Unique temp file per request (`tempfile` crate)
- [ ] Fix the 18 clippy warnings and remove dead code
- [ ] Add `ci.yml` (fmt + clippy + test on 3 OSes)
- [ ] Add a `LICENSE` file; fix the broken `docs/`, `licenses/` and `windows/` links in the README
- [ ] Update `MANIFEST.md` and `EXPORT.md` to match the tree (`harbor_adapter/` is missing)
- [ ] Panic hook that restores the terminal
- [ ] Error when a flag needs a value and gets none (`-p`, `--cwd`, `--max-turns`)
