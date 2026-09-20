#!/usr/bin/env bash
set -euo pipefail
base="${ICE_RELEASE_BASE_URL:-https://github.com/OWNER/ice-binaries/releases/latest/download}"; dest="${ICE_INSTALL_DIR:-$HOME/.local/bin}"; mkdir -p "$dest"; tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
case "$(uname -s):$(uname -m)" in Linux:x86_64) platform=bash;; Darwin:x86_64|Darwin:arm64) platform=mac;; *) echo "Unsupported platform" >&2; exit 2;; esac
curl --fail --location --silent --show-error "$base/$platform/ice" --output "$tmp/ice"; curl --fail --location --silent --show-error "$base/SHA256SUMS.txt" --output "$tmp/SHA256SUMS.txt"; expected="$(awk '$2 == "'"$platform/ice"'" {print $1; exit}' "$tmp/SHA256SUMS.txt")"; [[ "$expected" =~ ^[a-fA-F0-9]{64}$ ]] || { echo "No checksum for $platform/ice" >&2; exit 1; }; actual="$(sha256sum "$tmp/ice" | awk '{print $1}')"; [[ "$actual" == "$expected" ]] || { echo 'Checksum mismatch.' >&2; exit 1; }; install -m 0755 "$tmp/ice" "$dest/ice"; "$dest/ice" --version; echo "ICE installed to $dest/ice"
