#!/usr/bin/env bash
# macOS-only: builds the release binary and installs it to ~/Applications
# (which is on PATH), as `release`.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "install.sh is macOS-only." >&2
    exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$script_dir"

cargo build --release

install_dir="$HOME/Applications"
mkdir -p "$install_dir"
cp "target/release/release" "$install_dir/release"

echo "Installed $install_dir/release"
