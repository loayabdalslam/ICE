<p align="center">
  <img src="assets/hero.svg" width="100%" alt="ICE — Clarity in motion. Intent. Compile. Execute.">
</p>

<p align="center">
  <strong>Your next idea deserves a clear path to execution.</strong><br>
  A terminal agent workspace with a curious pixel companion, visible execution steps, and your choice of model provider.
</p>

<p align="center">
  <a href="#install"><img src="https://img.shields.io/badge/ICE-0.3.0-50d2ff?style=flat-square&labelColor=061018" alt="ICE 0.3.0"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-a0ecff?style=flat-square&labelColor=061018" alt="Apache 2.0 license"></a>
  <a href="#platforms"><img src="https://img.shields.io/badge/distribution-native_binaries-40dcaa?style=flat-square&labelColor=061018" alt="Native binary distribution"></a>
  <a href="#how-installation-works"><img src="https://img.shields.io/badge/downloads-SHA--256_verified-a0ecff?style=flat-square&labelColor=061018" alt="SHA-256 verified downloads"></a>
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#first-session">First session</a> ·
  <a href="#what-you-can-do">Features</a> ·
  <a href="#providers">Providers</a> ·
  <a href="#command-reference">Commands</a> ·
  <a href="#comparison-on-the-same-no-llm-metrics">Comparison</a> ·
  <a href="docs/WEBSITE.md">Website snippets</a>
</p>

<p align="center"><img src="assets/ice-preview.gif" width="960" alt="The actual ICE welcome screen with FLOE, an animated pixel ice cube with blinking eyes."></p>
<p align="center"><sub>FLOE Edition. Actual terminal-render captures. <a href="assets/welcome.png">Static preview</a> · <a href="assets/session.png">Conversation view</a></sub></p>

## Install

One command. A prebuilt binary. No Rust toolchain, project compilation, or administrator access required.

### <img src="assets/icons/windows.svg" width="20" alt="Windows"> Windows · PowerShell

```powershell
irm https://raw.githubusercontent.com/loayabdalslam/ICE/main/install.ps1 | iex
```

### <img src="assets/icons/ubuntu.svg" width="20" alt="Linux"> Linux · Bash

```bash
curl -fsSL https://raw.githubusercontent.com/loayabdalslam/ICE/main/bash/install.sh | bash
```

### <img src="assets/icons/apple.svg" width="20" alt="Apple"> macOS · Apple Silicon & Intel

```bash
curl -fsSL https://raw.githubusercontent.com/loayabdalslam/ICE/main/mac/install.sh | bash
```

Then start in your project folder:

```bash
ice
```

The same commands upgrade to the release named in [`LATEST`](LATEST). Open a new terminal after installation if `ice` is not yet on your PATH. For an offline first look, run `ice --demo`.

> This is the **binary distribution repository**. It contains executables, installers, documentation and artwork. Application source code, credentials and build caches are not included.

## What you can do

| | Feature | What it does |
| --- | --- | --- |
| <img src="assets/icons/terminal.svg" width="24" alt="Terminal"> | **A workspace that feels alive** | Meet FLOE: an isometric pixel ice cube with blinking eyes, gentle movement, and a glacier color palette. |
| <img src="assets/icons/diagram-3.svg" width="24" alt="Workflow"> | **Visible execution** | Follow intent, compiled action bursts, step results, assertions, and execution summaries in the terminal. |
| <img src="assets/icons/cpu.svg" width="24" alt="Models"> | **Choose your provider** | Configure hosted providers, local Ollama, or an OpenAI-compatible endpoint through onboarding. |
| <img src="assets/icons/check-circle.svg" width="24" alt="Tasks"> | **Keep work in view** | Use durable goals, todo items, skills, MCP tools and delegated agent tasks from slash commands. |
| <img src="assets/icons/download.svg" width="24" alt="Download"> | **A small installation** | Download the binary for your machine. No source checkout or package-manager runtime is needed to install. |
| <img src="assets/icons/shield-check.svg" width="24" alt="Integrity"> | **Verified downloads** | The installer checks the release checksum before executing a version check or replacing your installation. |

## Platforms

| Platform | Download | Verification |
| --- | --- | --- |
| Windows x64 | [Download](https://raw.githubusercontent.com/loayabdalslam/ICE/main/releases/0.2.0/windows-x86_64/ice.exe) | Executed and installed on Windows |
| Linux x64 | [Download](https://raw.githubusercontent.com/loayabdalslam/ICE/main/releases/0.2.0/linux-x86_64/ice) | Cross-compiled; native execution pending |
| Linux ARM64 | [Download](https://raw.githubusercontent.com/loayabdalslam/ICE/main/releases/0.2.0/linux-aarch64/ice) | Cross-compiled; native execution pending |
| macOS Intel | [Download](https://raw.githubusercontent.com/loayabdalslam/ICE/main/releases/0.2.0/macos-x86_64/ice) | Cross-compiled; native execution pending |
| macOS Apple Silicon | [Download](https://raw.githubusercontent.com/loayabdalslam/ICE/main/releases/0.2.0/macos-aarch64/ice) | Cross-compiled; native execution pending |

**Requirements:** a Unicode terminal; `curl` for API calls; Bash for shell actions. On Windows, install [Git for Windows](https://gitforwindows.org/) and make `bash.exe` available on PATH. Linux builds use musl to avoid a system glibc dependency. macOS builds require macOS 11 or newer. Windows ARM64 uses the x64 build through Windows emulation.

**Display:** 120 columns × 40 rows is recommended; the minimum is 36 × 14. Narrow layouts prioritize the composer and conversation. A Nerd Font is not required.

**Verification:** Windows execution and installation were exercised locally. Other platform binaries are cross-compiled; native execution status is recorded in [`docs/VERIFICATION.md`](docs/VERIFICATION.md). macOS binaries are not notarized, and Windows binaries are not Authenticode-signed.

## First session

1. Open a terminal in the project you want ICE to work with.
2. Run `ice`, choose a provider, enter its API key, and select a model.
3. Describe an outcome in the composer, then press Enter.

```text
Explain the structure of this project and identify its entry points.
```

Use Escape to skip onboarding and explore the interface, or `/onboard` to reopen it. Keys pasted into onboarding stay in the running process; they are not saved by the wizard. To reuse a key across sessions, configure the provider's environment variable yourself.

```bash
# Offline demo in a scratch directory
mkdir ice-playground
cd ice-playground
ice --demo
```

ICE creates `.ice/` workspace state in the selected directory. Demo execution runs predefined actions and can write demonstration files. Live mode can run shell commands and edit files in the working environment; it is not a virtual machine or OS sandbox.

## Providers

| Provider | Environment variable | Provider ID |
| --- | --- | --- |
| OpenAI | `OPENAI_API_KEY` | `openai` |
| Anthropic | `ANTHROPIC_API_KEY` | `anthropic` |
| Google Gemini | `GEMINI_API_KEY` or `GOOGLE_API_KEY` | `gemini` |
| xAI | `XAI_API_KEY` | `xai` |
| Groq | `GROQ_API_KEY` | `groq` |
| OpenRouter | `OPENROUTER_API_KEY` | `openrouter` |
| Together AI | `TOGETHER_API_KEY` | `together` |
| Fireworks AI | `FIREWORKS_API_KEY` | `fireworks` |
| DeepSeek | `DEEPSEEK_API_KEY` | `deepseek` |
| Mistral | `MISTRAL_API_KEY` | `mistral` |
| Ollama | `OLLAMA_API_KEY` / `ICE_API_KEY` | `ollama` |
| Custom compatible endpoint | `ICE_API_KEY` | `custom` |

These are adapters present in this release, not a guarantee that every service or model was tested. Model availability depends on your provider account. `/models` discovers models; `/model NAME` selects one. If your local server accepts unauthenticated requests, this release may still need a non-empty placeholder key for live-mode detection.

## Command reference

### Terminal commands

| Command | Purpose |
| --- | --- |
| `ice` | Open the interactive workspace |
| `ice --demo` | Explore the offline demo |
| `ice --cwd /path/to/project` | Select a workspace |
| `ice -p "your goal"` | Run a headless burst |
| `ice -p "your goal" --goal-loop --max-turns 4` | Run a bounded goal loop |
| `ice init /path/to/project` | Initialize `.ice/` workspace files |
| `ice inspect` | Print environment information |
| `ice --version` | Show the installed version |
| `ice --help` | Show CLI usage |

### Inside ICE

| Command | Purpose |
| --- | --- |
| `/onboard` | Provider, key and model setup |
| `/providers`, `/models`, `/model NAME` | Provider and model configuration |
| `/goal`, `/loop`, `/agent NAME: TASK` | Goals, loops and delegated tasks |
| `/todo Write a regression test`, `/todo done ID` | Add and complete a task |
| `/skills`, `/mcp` | Inspect installed skills and MCP tools |
| `/theme ice`, `/theme frost`, `/theme mono` | Change the palette; `groknight` and `ember` are also available |
| `/think` | Toggle visible thinking output |
| `/home`, `/new` | Return home or start a fresh session |
| `/demo`, `/live` | Switch execution mode |
| `/help`, `/exit` | Command guide and exit |

### Keyboard

`Enter` send · `Ctrl+L` command guide · `Esc` close/skip/back · `Ctrl+C` exit  
`←` / `→` edit · `Home` / `End` move cursor · `↑` / `↓` select or scroll · `Page Up` / `Page Down` scroll

## Comparison on the same no-LLM metrics

Only columns we actually timed for ICE. Other cells are **published
infrastructure numbers** for that same kind of work (startup, spawn,
tool dispatch, RAM, disk, patch, cancel). Empty cell = nobody published
that microbench. No Terminal-Bench / SWE-bench / model IQ in this table.

ICE row: this 2 vCPU box, 2026-09-15, engine path, no model.

| Metric | ICE (measured) | Claude Code | Codex CLI | OpenCode | Aider | Goose / others |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Startup / `--version` or engine import | **0.003 ms** import | 86–446 ms `--version`; TUI **3.5 s** to first frame | 35–38 ms `--version`; TUI input **152–250 ms**; catalog path **5.1 s** | TUI ready **1.05 s** | 5–20 s (Python + LiteLLM fetch) | jcode 49 ms; pi 596 ms; Copilot CLI 1.6 s; Cursor Agent 2.0 s |
| Process spawn `/bin/true` | **1.15 ms** | ~4 ms raw; bwrap+true **13 ms** | — | — | — | — |
| Tool dispatch `echo` / Bash noop | **4.8 ms** p50 | shell-fixed **65 ms**; sandbox off **~445 ms** total; sandbox on **8–16 s** stall | non-shell tool **~22 ms**; elevated Win sandbox **~88 s** before spawn | — | — | Copilot median tool **166 ms** |
| Parallel 100 noops | **106 ms** | parallel Bash + sandbox **15–16 s / call** in one report | — | — | — | — |
| File write 2k small | **141 ms** | — | — | — | `/drop *` on 19k-file repo: **minutes** | — |
| File read 2k | **26 ms** | — | — | — | — | Copilot `get_file` tens of ms |
| File search | **14 ms** `rg` | — | Seatbelt on every `rg` adds wall-clock in fork/exec traces | — | repo-map token pass, not timed | — |
| Patch 200 files | **18 ms** | Write/Edit usually seconds when healthy | `apply_patch` in traces (model-bound) | — | apply-patch is local after stream | Copilot `apply_patch` **250–600 ms** p50 |
| Diff 200 files | **6.7 ms** | — | — | — | live diff while streaming (model) | — |
| Git init+commit+branch | **37 ms** | `git status/diff/commit` **~5 s** when zsh startup was 1 s; **65 ms** after | — | — | uses git directly | — |
| JSON / IPC | **0.16 ms** | — | — | — | — | — |
| Context load ~143 KB src | **3.5 ms** | — | cold `~/.codex` I/O **1–5 min** if logs DB 419 MB | — | repo-map 1024 tok default | — |
| Timeout accuracy | **202 ms** on 200 ms cap | — | — | — | — | — |
| Cancel | **1.3 ms** | — | — | — | — | — |
| Isolation | two dirs, pass | sandbox / bwrap | Seatbelt / Landlock | session | git worktree | container / Harbor |
| Disk of harness state | **`.ice` 4.6 KB** | shell snapshots + session | `~/.codex` can hit **1 GB+** (419 MB sqlite + 625 MB sessions) | — | `.aider*` | — |
| Idle / bench RSS | **73 MB** process (Δ 0) | **166–192 MB** `--version` | **46 MB** `--version` | — | Python process, not published | Claw Rust CLI **4.1 MB** |
| Sandbox extra | none in this bench | bwrap prep **8–16 s**; binary itself 13 ms | Seatbelt **+49%** wall vs off in one ablation | — | none | Docker/microVM 90 ms–1 s typical cold |
| Cleanup 500 files | **0.16 ms** | — | — | — | — | — |
| Error propagate `exit 7` | **4.6 ms**, code 7 | — | — | — | — | — |
| Stream first chunk | **11 ms** | — | — | — | — | — |
| Backpressure 2 MB stdout | **17 ms** | — | — | — | — | — |

### Sources for the other columns

- Claude / Codex / Claw **startup + RAM + binary**: claw-bench on Ubuntu 24.04 (`--version`, idle RSS).
- TUI time-to-ready / first frame: jcode.sh PTY launch board (Claude Code 3513 ms, Codex 906 ms, OpenCode 1048 ms).
- Claude Code Bash dispatch: session timestamps 964 ms → 65 ms after shell-init fix; GitHub #56274 sandbox stall 8–16 s vs 80 ms spawn / 445 ms total with sandbox off; `/bin/true` ~4 ms, bwrap+true 13 ms.
- Codex TUI probes: upstream PR ~250 ms → 152 ms to accept input; issue #28877 `codex debug models` 5.1 s vs `--bundled` 60–80 ms; issue #28166 cold start 1–5 min on fat `~/.codex`.
- Codex Windows elevated sandbox: issue #31958, ~88 s before `powershell.exe`.
- Aider startup: issue #3111, 10–20 s (was <5 s), LiteLLM HTTP on boot.
- Copilot production traces: median tool 166 ms; `apply_patch` / `replace_string` 0.25–0.6 s.
- Git slowness under Claude Code: NIV tech, zsh -c true 1 s → 15 ms; Bash tool 964 ms → 65 ms.

Machines differ. Treat other-CLI cells as **published order of magnitude**, ICE cells as **this host**.

### What the matching columns say

1. **Dispatch, not spawn.** Raw `/bin/true` is 1–4 ms everywhere. Claude Code’s extra is snapshot + sandbox prep (65 ms healthy, seconds when sandbox path stalls). ICE’s extra on this host is ~4 ms.
2. **Startup is a product choice.** Rust `--version` is tens of ms (Codex 38 ms, Claw 1.2 ms). Node TUI first frame is 1–3.5 s (OpenCode / Claude Code). ICE engine import is microseconds; a full Rust TUI binary was not built here so we do not invent a first-frame number.
3. **State on disk bites later.** ICE `.ice` is 4.6 KB. Codex published a 1 GB+ `~/.codex` that turned cold start into minutes. That is a harness metric, not a model metric.
4. **Sandbox is the hidden tax.** Same Bash tool: 445 ms off vs 8–16 s on (Claude); Seatbelt +49% wall (Codex ablation); 88 s elevated Windows. ICE’s measured row has no container.
5. **Empty cells matter.** Nobody shipped a public “write 2k files / parse IR / cancel SIGTERM” number for Goose or OpenHands. We do not fill them with TB scores.

## Installation options

<details>
<summary><strong>Choose a version or installation directory</strong></summary>

Linux or macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/loayabdalslam/ICE/main/install.sh | bash -s -- --version 0.2.0 --bin-dir "$HOME/.local/bin"
```

Windows:

```powershell
$env:ICE_VERSION = '0.2.0'
$env:ICE_INSTALL_DIR = "$env:LOCALAPPDATA\Programs\ICE"
irm https://raw.githubusercontent.com/loayabdalslam/ICE/main/install.ps1 | iex
```

Defaults: Windows `%LOCALAPPDATA%\Programs\ICE`; Linux/macOS `~/.local/bin`. No `sudo` or administrator prompt is used. Shell installers configure Bash or Zsh startup files when needed; Windows updates your user PATH. Use `--no-path` or `-NoPath` to opt out.

</details>

<details>
<summary><strong>Download and inspect before installation</strong></summary>

```bash
curl -fsSL https://raw.githubusercontent.com/loayabdalslam/ICE/main/install.sh -o ice-install.sh
less ice-install.sh
bash ice-install.sh --no-path
```

```powershell
Invoke-WebRequest https://raw.githubusercontent.com/loayabdalslam/ICE/main/install.ps1 -OutFile ice-install.ps1
Get-Content .\ice-install.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\ice-install.ps1 -NoPath
```

`--download-only` / `-DownloadOnly` saves a verified binary without running it or changing PATH. `ICE_BASE_URL` supports an HTTPS mirror with the same repository layout. Binary checksums detect corruption and mismatched files; they are not independent code-signing signatures.

</details>

<details>
<summary><strong>Reduce motion</strong></summary>

```bash
ICE_REDUCED_MOTION=1 ice
```

```powershell
$env:ICE_REDUCED_MOTION = '1'
ice
```

</details>

<details>
<summary><strong>Uninstall</strong></summary>

Windows: download `install.ps1` as shown above, then run `powershell -NoProfile -ExecutionPolicy Bypass -File .\ice-install.ps1 -Uninstall`. Pass your original `-InstallDir` if you used a custom directory. This removes the installed executable and the PATH entry the installer added.

Linux/macOS: remove the installed `~/.local/bin/ice` file, or the `ice` file in your custom installation directory. You can also remove the `# ICE binary installation` line and its following export from `.bashrc`, `.bash_profile` or `.zshrc`. Workspace `.ice/` directories and provider configuration remain yours.

</details>

## How installation works

```text
LATEST → release version → operating system + CPU → binary → SHA-256 → install → ice
```

[`LATEST`](LATEST) contains a single version. [`releases/`](releases/) contains versioned executables and a `SHA256SUMS.txt` manifest per version. Installers select one binary; they do not clone application source or run a compiler. A missing platform, failed download or checksum mismatch stops installation before replacing an existing binary.

```text
LATEST
install.ps1                 Windows installer
install.sh                  Linux / macOS auto-detection
windows/install.ps1         Dedicated Windows entry point
bash/install.sh             Dedicated Linux entry point
mac/install.sh              Dedicated macOS entry point
releases/0.2.0/             Binaries + checksums
assets/                     Brand artwork + actual SVG icons
docs/                       Setup, verification and website snippets
licenses/                   Third-party notices
```

## Add ICE to your website

Copy one of the three commands in [Install](#install), or use the ready-to-paste buttons and command blocks in [`docs/WEBSITE.md`](docs/WEBSITE.md). The URLs stay the same across releases because the installers resolve `LATEST`.

## Troubleshooting

| Symptom | Next step |
| --- | --- |
| `ice` is not recognized | Open a new terminal, or run the binary by its full installed path. |
| `bash` is missing on Windows | Install Git for Windows and expose `bash.exe` on PATH. |
| A release or platform is unavailable | Check the platform table and `LATEST`; the installer does not substitute a binary from a different OS. |
| Checksum mismatch | Stop and re-download. Verify that your mirror contains a matching release manifest. |
| macOS blocks an unnotarized binary | Review the downloaded software using your normal macOS security workflow. The installer does not change Gatekeeper settings. |
| The model cannot connect | Reopen `/onboard`, check your provider key and network, then use `/models`. |
| Strange borders or missing glyphs | Use a Unicode terminal with a monospace font and sufficient viewport size. |

## Feedback & credits

[Report a bug](https://github.com/loayabdalslam/ICE/issues/new) with your ICE version, operating system, terminal and reproduction steps. Remove API keys and private project content from logs before sharing them.

ICE's application package declares the [Apache-2.0 license](LICENSE). Dependency notices are included in [`licenses/`](licenses/). The SVG UI and platform icons are from [Bootstrap Icons](https://github.com/twbs/icons), licensed under [MIT](assets/icons/LICENSE). FLOE and the ICE identity assets are included for presenting this distribution.

<p align="center"><img src="assets/floe.svg" width="72" alt="FLOE"><br><strong>Intent. Compile. Execute.</strong><br><sub>Built for the terminal. Made for your next idea.</sub></p>
