# Nyx Home

Native GTK 4 application for Home Assistant and Linux desktop automation.

- Use the public, pinned `nyx-core` and `nyx-design` crates. Do not import another
  application's internals.
- Preserve app identity `io.github.design_nexus.nyx.Home` and user-facing name
  `Home`.
- Reuse Nyx theme tokens, widgets, icons and shortcuts. Nyx Home uses ordinary
  sans-serif interface text; technical identifiers and values may use monospace.
- Home Assistant is authoritative for home data and home-critical automations.
- Never write credentials to TOML, SQLite, logs, argv or environment files. Use
  Secret Service and report unavailable/locked storage clearly.
- Keep provider traffic off the GTK main thread and redact errors before display.
- Run focused checks only. Do not download models or run inference for routine work.
- Install app updates without opening/restarting the app.
