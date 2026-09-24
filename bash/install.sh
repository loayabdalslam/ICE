#!/usr/bin/env bash
ICE_EXPECT_OS=Linux
# ICE installer for Linux and macOS (Bash 3.2+, including macOS system Bash).
#
# Default: clone the ICE repository, build it with cargo, and install the
# binary. Re-running it (or `ice update`) pulls the latest commits and
# rebuilds. Use --binary to install a prebuilt, SHA-256-verified release
# instead of building from source.
#
#   curl -fsSL https://raw.githubusercontent.com/loayabdalslam/ICE/main/install.sh | bash
#   curl -fsSL .../install.sh | bash -s -- --ref v0.5.0 --bin-dir ~/bin
set -euo pipefail

ice_main() {
  local repo="${ICE_REPO:-https://github.com/loayabdalslam/ICE.git}"
  local ref="${ICE_REF:-main}"
  local base="${ICE_BASE_URL:-https://raw.githubusercontent.com/loayabdalslam/ICE/main}"
  local data_home="${XDG_DATA_HOME:-$HOME/.local/share}"
  local src_dir="${ICE_SOURCE_DIR:-$data_home/ice/src}"
  local bin_dir="${ICE_INSTALL_DIR:-$HOME/.local/bin}"
  local config_dir="${ICE_CONFIG_DIR:-$HOME/.ice}"
  local version="${ICE_VERSION:-}"
  local no_path=0 binary=0 no_rustup=0 print_target=0
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --ref|--repo|--dir|--bin-dir|--version)
        [ "$#" -ge 2 ] || { printf 'Missing value for %s\n' "$1" >&2; return 2; }
        case "$1" in
          --ref) ref="$2" ;; --repo) repo="$2" ;; --dir) src_dir="$2" ;;
          --bin-dir) bin_dir="$2" ;; --version) version="$2"; binary=1 ;;
        esac
        shift 2 ;;
      --binary) binary=1; shift ;;
      --no-path) no_path=1; shift ;;
      --no-rustup) no_rustup=1; shift ;;
      --print-target) print_target=1; shift ;;
      --help|-h)
        cat <<'EOF'
ICE installer

  install.sh [options]

Options:
  --ref REF         Branch, tag or commit to build (default: main)
  --repo URL        Git repository (default: https://github.com/loayabdalslam/ICE.git)
  --dir DIR         Where the source checkout lives (default: ~/.local/share/ice/src)
  --bin-dir DIR     Where the ice binary is installed (default: ~/.local/bin)
  --binary          Install a prebuilt release binary instead of building
  --version X.Y.Z   Prebuilt release to install (implies --binary)
  --no-rustup       Don't install Rust automatically if cargo is missing
  --no-path         Don't edit your shell profile
EOF
        return ;;
      *) printf 'Unknown option: %s (see --help)\n' "$1" >&2; return 2 ;;
    esac
  done

  local os machine
  os="$(uname -s)"; machine="$(uname -m)"
  if [ -n "${ICE_EXPECT_OS:-}" ] && [ "$os" != "$ICE_EXPECT_OS" ]; then
    printf 'This installer is for %s. Use the installer for your operating system.\n' "$ICE_EXPECT_OS" >&2; return 2
  fi
  case "$os" in Linux) os=linux ;; Darwin) os=macos ;; *) printf 'Use install.ps1 on Windows; this script supports Linux and macOS.\n' >&2; return 2 ;; esac
  case "$machine" in x86_64|amd64) machine=x86_64 ;; aarch64|arm64) machine=aarch64 ;; *) printf 'Unsupported architecture: %s\n' "$machine" >&2; return 2 ;; esac
  local target="$os-$machine"
  if [ "$print_target" -eq 1 ]; then printf '%s\n' "$target"; return; fi

  case "$bin_dir" in *$'\n'*|*$'\r'*) printf 'Invalid installation directory.\n' >&2; return 2 ;; esac
  mkdir -p -- "$bin_dir" "$config_dir"
  bin_dir="$(cd "$bin_dir" && pwd -P)"

  printf '\n  \033[36m■ ICE\033[0m  Intent. Compile. Execute.\n'

  local built=""
  if [ "$binary" -eq 0 ]; then
    command -v git >/dev/null 2>&1 || {
      printf '\ngit is required to install ICE from source.\n' >&2
      if [ "$os" = macos ]; then printf 'Install it with: xcode-select --install\n' >&2
      else printf 'Install it with your package manager, e.g.: sudo apt install git build-essential\n' >&2; fi
      printf 'Or install a prebuilt binary: install.sh --binary\n' >&2
      return 1
    }

    # 1. Get the source: clone once, then fast-forward on later runs.
    printf '  Source   %s (%s)\n  Into     %s\n\n' "$repo" "$ref" "$src_dir"
    if [ -d "$src_dir/.git" ]; then
      git -C "$src_dir" remote set-url origin "$repo"
      if ! { git -C "$src_dir" fetch --quiet --depth 1 origin "$ref" && git -C "$src_dir" checkout --quiet --force FETCH_HEAD; }; then
        printf 'Updating the existing checkout failed; cloning again.\n'
        rm -rf -- "$src_dir"
      fi
    fi
    if [ ! -d "$src_dir/.git" ]; then
      mkdir -p -- "$(dirname "$src_dir")"
      git clone --quiet --depth 1 --branch "$ref" "$repo" "$src_dir" 2>/dev/null || {
        # A commit id can't be cloned by --branch: clone, then fetch it.
        git clone --quiet --depth 1 "$repo" "$src_dir"
        git -C "$src_dir" fetch --quiet --depth 1 origin "$ref"
        git -C "$src_dir" checkout --quiet --force FETCH_HEAD
      }
    fi
    local commit
    commit="$(git -C "$src_dir" rev-parse --short HEAD)"

    # 2. Get a Rust toolchain.
    if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
      export PATH="$HOME/.cargo/bin:$PATH"
    fi
    if ! command -v cargo >/dev/null 2>&1; then
      if [ "$no_rustup" -eq 1 ]; then
        printf 'cargo was not found. Install Rust from https://rustup.rs and run this again.\n' >&2; return 1
      fi
      command -v curl >/dev/null 2>&1 || { printf 'curl is required to install Rust.\n' >&2; return 1; }
      printf '  Rust     not found · installing the minimal toolchain with rustup…\n'
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path >/dev/null
      export PATH="$HOME/.cargo/bin:$PATH"
    fi
    if ! command -v cc >/dev/null 2>&1 && ! command -v gcc >/dev/null 2>&1 && ! command -v clang >/dev/null 2>&1; then
      printf 'A C linker is required to build Rust programs.\n' >&2
      if [ "$os" = macos ]; then printf 'Install it with: xcode-select --install\n' >&2
      else printf 'Install it with: sudo apt install build-essential (or your distro equivalent)\n' >&2; fi
      return 1
    fi

    # 3. Build.
    printf '  Build    cargo build --release (%s) — the first build takes a few minutes…\n' "$commit"
    if ! (cd "$src_dir" && cargo build --release --locked --quiet); then
      printf '\nThe build failed. Your previous installation (if any) was not changed.\n' >&2
      printf 'Try again, or install a prebuilt binary with: install.sh --binary\n' >&2
      return 1
    fi
    built="$src_dir/target/release/ice"
  else
    # Prebuilt release, verified against SHA256SUMS.txt.
    command -v curl >/dev/null 2>&1 || { printf 'curl is required.\n' >&2; return 2; }
    local hash_tool
    if command -v sha256sum >/dev/null 2>&1; then hash_tool=sha256sum
    elif command -v shasum >/dev/null 2>&1; then hash_tool=shasum
    else printf 'A SHA-256 tool (sha256sum or shasum) is required.\n' >&2; return 2; fi
    local protocol='=https'
    case "$base" in
      https://*) ;;
      http://127.0.0.1:*|http://localhost:*) protocol='=http,https' ;;
      *) printf 'ICE_BASE_URL must use HTTPS (HTTP is allowed only on loopback for local tests).\n' >&2; return 2 ;;
    esac
    base="${base%/}"
    local tmp
    tmp="$(mktemp -d "${TMPDIR:-/tmp}/ice-install.XXXXXXXX")"
    trap 'rm -rf -- "$tmp"' EXIT
    fetch() { curl --fail --location --silent --show-error --proto "$protocol" --proto-redir "$protocol" --retry 2 --connect-timeout 15 --max-time 180 "$1" --output "$2"; }
    if [ -z "$version" ]; then
      fetch "$base/LATEST" "$tmp/LATEST" || { printf 'Cannot read LATEST.\n' >&2; return 1; }
      version="$(tr -d '\r' < "$tmp/LATEST")"
    fi
    [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { printf 'Invalid release version: %s\n' "$version" >&2; return 1; }
    printf '  Release  %s for %s\n' "$version" "$target"
    fetch "$base/releases/$version/SHA256SUMS.txt" "$tmp/SHA256SUMS.txt" || { printf 'Release %s is unavailable.\n' "$version" >&2; return 1; }
    local expected actual
    expected="$(awk -v asset="$target/ice" '$2 == asset {print $1}' "$tmp/SHA256SUMS.txt" | tr -d '\r')"
    [[ "$expected" =~ ^[a-fA-F0-9]{64}$ ]] || { printf 'No prebuilt binary for %s in %s. Install from source instead (drop --binary).\n' "$target" "$version" >&2; return 1; }
    fetch "$base/releases/$version/$target/ice" "$tmp/ice" || { printf 'Binary download failed.\n' >&2; return 1; }
    if [ "$hash_tool" = sha256sum ]; then actual="$(sha256sum "$tmp/ice" | awk '{print $1}')"
    else actual="$(shasum -a 256 "$tmp/ice" | awk '{print $1}')"; fi
    [ "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" = "$actual" ] || { printf 'SHA-256 mismatch. Installation stopped.\n' >&2; return 1; }
    chmod 755 "$tmp/ice"
    built="$tmp/ice"
  fi

  # 4. Install atomically, after checking the binary runs.
  "$built" --version >/dev/null || { printf 'The built binary cannot run on this machine; nothing was changed.\n' >&2; return 1; }
  local stage
  stage="$(mktemp "$bin_dir/.ice-new.XXXXXXXX")"
  if ! cp "$built" "$stage" || ! chmod 755 "$stage" || ! mv -f "$stage" "$bin_dir/ice"; then
    rm -f -- "$stage"; printf 'Could not install ICE in %s.\n' "$bin_dir" >&2; return 1
  fi
  if [ "$binary" -eq 0 ]; then
    printf '{\n  "method": "source",\n  "source_dir": "%s",\n  "ref": "%s",\n  "bin": "%s"\n}\n' "$src_dir" "$ref" "$bin_dir/ice" > "$config_dir/install.json"
  else
    printf '{\n  "method": "binary",\n  "version": "%s",\n  "bin": "%s"\n}\n' "$version" "$bin_dir/ice" > "$config_dir/install.json"
  fi

  # 5. PATH.
  if [ "$no_path" -eq 0 ]; then
    local profile='' path_line
    case "${SHELL:-/bin/bash}" in
      */zsh) profile="$HOME/.zshrc" ;;
      */bash) if [ "$os" = macos ]; then profile="$HOME/.bash_profile"; else profile="$HOME/.bashrc"; fi ;;
      */fish) profile="" ; printf 'fish: run  fish_add_path %s\n' "$bin_dir" ;;
    esac
    printf -v path_line 'export PATH=%q:"$PATH"' "$bin_dir"
    case ":$PATH:" in *":$bin_dir:"*) ;;
      *) if [ -n "$profile" ]; then
           if [ ! -f "$profile" ] || ! grep -Fqx -- "$path_line" "$profile"; then
             printf '\n# ICE\n%s\n' "$path_line" >> "$profile"
           fi
           printf '  PATH     added %s to %s (open a new terminal)\n' "$bin_dir" "$profile"
         fi ;;
    esac
  fi

  printf '\n  \033[32m✔\033[0m Installed %s → %s\n' "$("$bin_dir/ice" --version)" "$bin_dir/ice"
  printf '\n  Get started:\n    cd your-project\n    ice\n\n  Update any time with:  ice update\n\n'
}

ice_main "$@"
