# Home

Nixe Home is a native Linux frontend for an existing Home Assistant instance, with a
separate desktop automation engine. Home Assistant remains
the smart-home backend.

## Current functionality

- Connect to Home Assistant with a long-lived access token.
- Store the credential in the desktop Secret Service (through `secret-tool`), never
  in the settings file.
- Discover entities, areas, and devices; show live state changes and group entities
  by area (with unassigned entities grouped by domain).
- Pin devices to Overview and use basic controls for lights, switches, fans, input
  booleans, and locks.
- Run desktop automations from a systemd user service, independently of the window.
- Build desktop or Home Assistant automations with a native WHEN → IF → THEN
  editor. Add multiple actions, edit saved workflows, dry run, and live test.
  Enable or disable workflows from the Automations page and review desktop run
  results in Activity.
- Store versioned desktop workflows and execution history in SQLite. Supported
  triggers include manual, startup, intervals, and Hyprland window open/close;
  actions include notifications, desktop application launch, workspace switching,
  and volume.

The current build does not yet support cameras, robot controls, the advanced node
canvas, or hybrid workflows.
OAuth is deferred until the app has a registered website redirect. See
[the architecture notes](docs/ARCHITECTURE.md).

## Build and install

Requirements: Rust/Cargo, GTK 4.18+, and `secret-tool` from libsecret.

```sh
cargo build --release --locked
./install.sh
```

This installs `nixe-home`, `nixe-homed`, `nixe-homectl`, a launcher entry, icon, and
an enabled systemd user service under `~/.local`. It does not open the app. To run it, use `nixe-home` or select **Home** in
the application launcher.

## Connect

Open **Home** and choose **Connect Home Assistant**. Enter the base URL (for example
`http://homeassistant.local:8123`) and a long-lived access token created in your
Home Assistant profile. Nixe Home stores the token in Secret Service. If your keyring
is locked or unavailable, unlock or enable it before connecting.

Use HTTPS when connecting over an untrusted network. Home Assistant authentication
and service permissions remain managed by Home Assistant.

## Desktop automations

The Automations page provides desktop and Home Assistant builders. Home Assistant
automations currently support an entity state trigger, an optional entity state
condition, and on/off or scene actions. Home Assistant executes these workflows;
the desktop service keeps their editable Nixe graph but never runs them.

The service also accepts versioned desktop workflow JSON through `nixe-homectl save FILE` for supported
desktop triggers and actions. Use `nixe-homectl list`, `run ID`, `enable ID`,
`disable ID`, `history`, `status`, and `dry-run FILE [CONTEXT]` to manage or inspect
desktop workflows while the GUI is closed.
An [example workflow](examples/manual-notification.json) can be imported with
`nixe-homectl save examples/manual-notification.json`.
The workflow file has `schema_version`, `id`, `name`, `enabled`, `nodes`, and `edges`.
Each node has an `id` and a `type`; edges connect `from` to `to`. The simple builder supports
one trigger, an optional condition, and a linear chain of actions. Workflow validation rejects
unknown node types, unsafe application IDs, cycles, and disconnected nodes.

The service runs as the current user with no root privileges. It only invokes
allowlisted commands with separate arguments, enforces action timeouts, and records
completion or failure. Desktop automation data is stored at
`~/.local/share/nixe-home/automation.db`. Run `systemctl --user status nixe-homed`
to inspect the service. Hyprland event triggers require an active Hyprland session.

## Development

```sh
cargo check --locked
cargo test --locked
```

The app uses the pinned Nixe `nixe-core` and `nixe-design` crates. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for boundaries and current limitations.
