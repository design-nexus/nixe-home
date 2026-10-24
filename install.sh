#!/usr/bin/env bash
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
command -v cargo >/dev/null || { echo "Rust/Cargo is required." >&2; exit 1; }

cargo build --release --locked --manifest-path "$root/Cargo.toml"
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
share_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
install -d "$bin_dir"
install_binary() {
  local source="$1" target="$2" staged
  staged="$(mktemp "$target.XXXXXX")"
  install -m755 "$source" "$staged"
  mv -f "$staged" "$target"
}
install_binary "$root/target/release/nixe-home" "$bin_dir/nixe-home"
install_binary "$root/target/release/nixe-homed" "$bin_dir/nixe-homed"
install_binary "$root/target/release/nixe-homectl" "$bin_dir/nixe-homectl"
install -Dm644 "$root/data/io.github.design_nexus.nixe.Home.desktop" \
  "$share_dir/applications/io.github.design_nexus.nixe.Home.desktop"
install -Dm644 "$root/data/icons/io.github.design_nexus.nixe.Home.svg" \
  "$share_dir/icons/hicolor/scalable/apps/io.github.design_nexus.nixe.Home.svg"
install -Dm644 "$root/data/nixe-homed.service" "$share_dir/systemd/user/nixe-homed.service"
systemctl --user daemon-reload
systemctl --user enable nixe-homed.service
systemctl --user start nixe-homed.service
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t "$share_dir/icons/hicolor" || true
fi
printf 'Installed Home to %s\n' "$bin_dir/nixe-home"
