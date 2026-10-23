#!/usr/bin/env bash
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
command -v cargo >/dev/null || { echo "Rust/Cargo is required." >&2; exit 1; }

cargo build --release --locked --manifest-path "$root/Cargo.toml"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
share_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
install -Dm755 "$root/target/release/nixe-home" "$bin_dir/nixe-home"
install -Dm644 "$root/data/io.github.design_nexus.nixe.Home.desktop" \
  "$share_dir/applications/io.github.design_nexus.nixe.Home.desktop"
install -Dm644 "$root/data/icons/io.github.design_nexus.nixe.Home.svg" \
  "$share_dir/icons/hicolor/scalable/apps/io.github.design_nexus.nixe.Home.svg"
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t "$share_dir/icons/hicolor" || true
fi
printf 'Installed Home to %s\n' "$bin_dir/nixe-home"
