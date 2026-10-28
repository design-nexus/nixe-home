#!/usr/bin/env bash
# Remove Home's programs, launcher entry, icon and desktop service. Settings,
# automations and the keyring token are kept unless --purge is given.
set -euo pipefail
bin_dir="${XDG_BIN_HOME:-$HOME/.local/bin}"
share_dir="${XDG_DATA_HOME:-$HOME/.local/share}"
if systemctl --user show-environment >/dev/null 2>&1; then
  systemctl --user disable --now nyx-homed.service >/dev/null 2>&1 || true
fi
rm -f "$bin_dir/nyx-home" "$bin_dir/nyx-homed" "$bin_dir/nyx-homectl" \
  "$share_dir/applications/io.github.design_nexus.nyx.Home.desktop" \
  "$share_dir/icons/hicolor/scalable/apps/io.github.design_nexus.nyx.Home.svg" \
  "$share_dir/systemd/user/nyx-homed.service" \
  "$share_dir/dbus-1/services/io.github.design_nexus.nyx.Home.Automation.service"
if systemctl --user show-environment >/dev/null 2>&1; then
  systemctl --user daemon-reload
fi
if [[ ${1:-} == --purge ]]; then
  rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/nyx-home" "$share_dir/nyx-home"
  if command -v secret-tool >/dev/null 2>&1; then
    secret-tool clear service nyx-home account home-assistant 2>/dev/null || true
    secret-tool clear service nixe-home account home-assistant 2>/dev/null || true
  fi
  echo "Removed Home, its settings, automations and saved token."
else
  echo "Removed Home. Settings and automations are kept; run with --purge to remove them too."
fi
