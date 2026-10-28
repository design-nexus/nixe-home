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
# The unit and the D-Bus activation file name the daemon by its installed path.
install_with_bindir() {
  local source="$1" target="$2"
  install -d "$(dirname "$target")"
  sed "s|@BINDIR@|$bin_dir|g" "$source" >"$target.tmp"
  chmod 644 "$target.tmp"
  mv -f "$target.tmp" "$target"
}
install_binary "$root/target/release/nyx-home" "$bin_dir/nyx-home"
install_binary "$root/target/release/nyx-homed" "$bin_dir/nyx-homed"
install_binary "$root/target/release/nyx-homectl" "$bin_dir/nyx-homectl"
install -Dm644 "$root/data/io.github.design_nexus.nyx.Home.desktop" \
  "$share_dir/applications/io.github.design_nexus.nyx.Home.desktop"
install -Dm644 "$root/data/icons/io.github.design_nexus.nyx.Home.svg" \
  "$share_dir/icons/hicolor/scalable/apps/io.github.design_nexus.nyx.Home.svg"
install_with_bindir "$root/data/nyx-homed.service" "$share_dir/systemd/user/nyx-homed.service"
install_with_bindir "$root/data/io.github.design_nexus.nyx.Home.Automation.service" \
  "$share_dir/dbus-1/services/io.github.design_nexus.nyx.Home.Automation.service"
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t "$share_dir/icons/hicolor" || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q "$share_dir/applications" || true
fi
# The desktop service needs a user session; without one, it starts on login.
if systemctl --user show-environment >/dev/null 2>&1; then
  systemctl --user daemon-reload
  systemctl --user enable nyx-homed.service >/dev/null
  # Restart so an upgrade runs the new daemon; the app itself is left alone.
  systemctl --user restart nyx-homed.service
else
  echo "No user session bus: the desktop service will start when you next log in." >&2
fi
printf 'Installed Home to %s\n' "$bin_dir/nyx-home"
