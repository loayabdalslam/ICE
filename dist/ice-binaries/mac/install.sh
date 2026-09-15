#!/usr/bin/env bash
ICE_EXPECT_OS=Darwin
# ICE installer. Compatible with Bash 3.2+ (including the macOS system Bash).
set -euo pipefail

ice_main() {
  local base="${ICE_BASE_URL:-https://raw.githubusercontent.com/loayabdalslam/ice-binaries/main}"
  local version="${ICE_VERSION:-}" bin_dir="${ICE_INSTALL_DIR:-$HOME/.local/bin}"
  local no_path=0 download_only=0 print_target=0
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --version|--bin-dir)
        [ "$#" -ge 2 ] || { printf 'Missing value for %s\n' "$1" >&2; return 2; }
        if [ "$1" = --version ]; then version="$2"; else bin_dir="$2"; fi
        shift 2 ;;
      --no-path) no_path=1; shift ;;
      --download-only) download_only=1; no_path=1; shift ;;
      --print-target) print_target=1; shift ;;
      --help|-h)
        printf '%s\n' 'ICE: install.sh [--version X.Y.Z] [--bin-dir DIR] [--no-path] [--download-only] [--print-target]'
        return ;;
      *) printf 'Unknown option: %s\n' "$1" >&2; return 2 ;;
    esac
  done
  local os machine target
  os="$(uname -s)"; machine="$(uname -m)"
  if [ -n "${ICE_EXPECT_OS:-}" ] && [ "$os" != "$ICE_EXPECT_OS" ]; then
    printf 'This installer is for %s. Use the installer for your operating system.\n' "$ICE_EXPECT_OS" >&2; return 2
  fi
  case "$os" in Linux) os=linux ;; Darwin) os=macos ;; *) printf 'Use install.ps1 on Windows; this script supports Linux and macOS.\n' >&2; return 2 ;; esac
  case "$machine" in x86_64|amd64) machine=x86_64 ;; aarch64|arm64) machine=aarch64 ;; *) printf 'Unsupported architecture: %s\n' "$machine" >&2; return 2 ;; esac
  target="$os-$machine"
  if [ "$print_target" -eq 1 ]; then printf '%s\n' "$target"; return; fi
  command -v curl >/dev/null 2>&1 || { printf 'curl is required. Install curl and run this command again.\n' >&2; return 2; }
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
  ice_fetch() { curl --fail --location --silent --show-error --proto "$protocol" --proto-redir "$protocol" --retry 2 --connect-timeout 15 --max-time 180 "$1" --output "$2"; }
  local ice_tmp
  ice_tmp="$(mktemp -d "${TMPDIR:-/tmp}/ice-install.XXXXXXXX")"
  # This path is returned directly by mktemp, never supplied by a download.
  trap 'rm -rf -- "$ice_tmp"' EXIT
  trap 'exit 130' INT
  trap 'exit 143' TERM
  if [ -z "$version" ]; then
    ice_fetch "$base/LATEST" "$ice_tmp/LATEST" || { printf 'Cannot read LATEST. Check the repository URL or network.\n' >&2; exit 1; }
    version="$(tr -d '\r' < "$ice_tmp/LATEST")"
  fi
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { printf 'Invalid release version: %s\n' "$version" >&2; exit 1; }
  local release="$base/releases/$version" expected actual
  printf '\n  ICE / Intent. Compile. Execute.\n  Installing %s for %s\n\n' "$version" "$target"
  ice_fetch "$release/SHA256SUMS.txt" "$ice_tmp/SHA256SUMS.txt" || { printf 'Release %s is unavailable.\n' "$version" >&2; exit 1; }
  expected="$(awk -v asset="$target/ice" '$2 == asset {print $1}' "$ice_tmp/SHA256SUMS.txt" | tr -d '\r')"
  [[ "$expected" =~ ^[a-fA-F0-9]{64}$ ]] || { printf 'No verified binary is published for %s in ICE %s.\n' "$target" "$version" >&2; exit 1; }
  ice_fetch "$release/$target/ice" "$ice_tmp/ice" || { printf 'Binary download failed; your installation was not changed.\n' >&2; exit 1; }
  if [ "$hash_tool" = sha256sum ]; then actual="$(sha256sum "$ice_tmp/ice" | awk '{print $1}')"
  else actual="$(shasum -a 256 "$ice_tmp/ice" | awk '{print $1}')"; fi
  [ "$(printf '%s' "$expected" | tr 'A-F' 'a-f')" = "$actual" ] || { printf 'SHA-256 mismatch. Installation stopped.\n' >&2; exit 1; }
  chmod 755 "$ice_tmp/ice"
  if [ "$download_only" -eq 0 ]; then
    "$ice_tmp/ice" --version || { printf 'The downloaded binary cannot run on this machine; your installation was not changed.\n' >&2; exit 1; }
  fi
  case "$bin_dir" in *$'\n'*|*$'\r'*) printf 'Invalid installation directory.\n' >&2; exit 2 ;; esac
  mkdir -p -- "$bin_dir"
  bin_dir="$(cd "$bin_dir" && pwd -P)"
  local stage
  stage="$(mktemp "$bin_dir/.ice-new.XXXXXXXX")"
  if ! cp "$ice_tmp/ice" "$stage" || ! chmod 755 "$stage" || ! mv -f "$stage" "$bin_dir/ice"; then
    rm -f -- "$stage"; printf 'Could not install ICE in %s.\n' "$bin_dir" >&2; exit 1
  fi
  if [ "$no_path" -eq 0 ]; then
    local profile='' path_line
    case "${SHELL:-/bin/bash}" in
      */zsh) profile="$HOME/.zshrc" ;;
      */bash) if [ "$os" = macos ]; then profile="$HOME/.bash_profile"; else profile="$HOME/.bashrc"; fi ;;
    esac
    printf -v path_line 'export PATH=%q:"$PATH"' "$bin_dir"
    case ":$PATH:" in *":$bin_dir:"*) ;;
      *) if [ -n "$profile" ]; then
           if [ ! -f "$profile" ] || ! grep -Fqx -- "$path_line" "$profile"; then
             printf '\n# ICE binary installation\n%s\n' "$path_line" >> "$profile"
           fi
           printf 'PATH configured in %s. Open a new terminal to use ice.\n' "$profile"
         else printf 'Add this to your shell configuration: %s\n' "$path_line"; fi ;;
    esac
  fi
  printf '\nInstalled: %s/ice\nStart:     "%s/ice" --demo\n' "$bin_dir" "$bin_dir"
  rm -rf -- "$ice_tmp"
  trap - EXIT INT TERM
}

ice_main "$@"
