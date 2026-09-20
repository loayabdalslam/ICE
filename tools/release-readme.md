# ICE 0.2.0 / FLOE Edition

**Intent. Compile. Execute.** A terminal agent workspace with an animated pixel
ice companion, responsive panels, provider onboarding and a focused composer.

## Install — Windows x64

From the distribution root (the folder containing `install.ps1`):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\install.ps1
```

Open a new terminal and run `ice`, or launch ICE from the Start menu. No Rust
toolchain or administrator access is needed. Default destination:
`%LOCALAPPDATA%\Programs\ICE`. The installer verifies SHA-256 checksums, adds
the install directory to your user PATH and creates a branded shortcut.

Portable use: run `build\ice.exe --demo` directly. For a custom installation:

```powershell
.\install.ps1 -InstallDir C:\Tools\ICE -NoPath -NoShortcut
```

Install into an empty directory or a directory previously managed by this installer.
An existing installation can be updated by running the installer again.

## First run

Choose a provider with Up/Down and Enter. Paste a key in the masked input, then
choose a model. Escape skips setup; `/onboard` reopens it. API keys entered in
the wizard are process-only. Set your provider's environment variable to keep
using it across sessions. Provider/model preferences are saved by ICE.

```powershell
ice --demo
ice --cwd C:\Work\project
ice -p "inspect this workspace" --demo
ice init C:\Work\project
ice --version
```

ICE initializes `.ice` state in the selected working directory. Headless demo
can create demonstration files; use a scratch directory when exploring it.

The TUI supports Windows Terminal and other Unicode/true-color terminals.
Minimum viewport: 36×14. Recommended: 120×40. Wide layouts show the companion
and workspace sidebar; compact layouts prioritize content and input.

Shell execution uses `bash.exe`: install Git for Windows and make Bash available
on PATH. Live provider calls use `curl.exe` (included in current Windows).
Provider access also requires a valid API key and network access.

## Controls

| Key / command | Action |
| --- | --- |
| Enter | Send / advance setup |
| Left / Right, Home / End | Move editor cursor |
| Up / Down | Scroll / select setup item |
| Page Up / Page Down | Scroll conversation by ten lines |
| Ctrl+L | Open / close command guide |
| Escape | Close guide / skip setup / return home with empty input |
| Ctrl+C | Exit |
| `/theme ice`, `/theme mono` | Change palette |
| `/providers`, `/models`, `/model NAME` | Model configuration |
| `/todo`, `/skills`, `/mcp`, `/goal` | Workspace tools |

Disable motion before launching:

```powershell
$env:ICE_REDUCED_MOTION = '1'
ice
```

## Uninstall

```powershell
.\install.ps1 -Uninstall
```

For a custom location, pass the same `-InstallDir`. Only installer-owned files,
its shortcut and PATH entry are removed. Workspace and provider data remain.

## Identity and verification

Open `brand/preview.html` for the animated UI review and `brand/BRAND.md` for
the identity system. SVG logos, PNG artwork and a multi-size Windows icon are
included. The executable also contains the FLOE icon and Windows version metadata.

This is a Windows x64 release; Linux/macOS binaries are not included.
The package is not Authenticode-signed. Live paid provider calls are not part
of the offline release tests. See `licenses/` for dependency notices.
