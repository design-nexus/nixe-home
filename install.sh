#!/usr/bin/env bash
set -euo pipefail
# Piped from curl (`curl -fsSL …/install.sh | bash`): no checkout here, so fetch
# the source and run its installer from there.
if [[ -z ${BASH_SOURCE[0]:-} ]]; then
  command -v git >/dev/null || { echo "Git is needed to install nyx-home." >&2; exit 1; }
  work=$(mktemp -d)
  trap 'rm -rf "$work"' EXIT
  git clone -q --depth 1 https://github.com/design-nexus/nyx-home.git "$work/src"
  bash "$work/src/install.sh" "$@"
  exit
fi

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
# A release archive (.prebuilt) brings its binaries.
if [[ ! -f $root/.prebuilt ]]; then
  command -v cargo >/dev/null || { echo "Rust/Cargo is required." >&2; exit 1; }
  cargo build --release --locked --manifest-path "$root/Cargo.toml"
fi
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
share_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
install -d "$bin_dir"
install_binary() {
  local source="$1" target="$2" staged
  staged="$(mktemp "$target.XXXXXX")"
  install -m755 "$source" "$staged"
  mv -f "$staged" "$target"
}
install_binary "$root/target/release/nyx-home" "$bin_dir/nyx-home"
install_binary "$root/target/release/nyx-homed" "$bin_dir/nyx-homed"
install_binary "$root/target/release/nyx-homectl" "$bin_dir/nyx-homectl"
install -Dm644 "$root/data/io.github.design_nexus.nyx.Home.desktop" \
  "$share_dir/applications/io.github.design_nexus.nyx.Home.desktop"
install -Dm644 "$root/data/icons/io.github.design_nexus.nyx.Home.svg" \
  "$share_dir/icons/hicolor/scalable/apps/io.github.design_nexus.nyx.Home.svg"
install -Dm644 "$root/data/nyx-homed.service" "$share_dir/systemd/user/nyx-homed.service"
systemctl --user daemon-reload
systemctl --user enable nyx-homed.service
systemctl --user start nyx-homed.service
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t "$share_dir/icons/hicolor" || true
fi
printf 'Installed Home to %s\n' "$bin_dir/nyx-home"
