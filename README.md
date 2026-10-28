# Home

Nyx Home is a native Linux frontend for an existing Home Assistant instance, with a
separate desktop automation engine. Home Assistant remains the smart-home backend.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/design-nexus/nyx-home/main/install.sh | bash
```

## What it does

- **Overview:** lights on, open doors and windows, who's home, the indoor
  temperature and device health at a glance, your favorite devices as live
  tiles, and cameras, robots and energy sensors when Home Assistant has them.
- **Rooms:** every device grouped by Home Assistant area, with search and an
  area filter. Tiles have the control that fits the device: a switch, a
  brightness slider, target temperature, cover and media buttons, a two-click
  unlock, robot commands, or a camera still.
- **Automations:** desktop automations that run in a user service even with the
  window closed, Home Assistant automations that run on your server, and hybrid
  automations that connect the two. Build them with a simple When / Only if /
  Then builder or on a canvas with branches, parallel paths, waits and retries.
- **Activity:** what desktop and hybrid automations did, step by step.
- **Connections:** the Home Assistant server and token, and the desktop service.

The foot of the sidebar shows whether Home is connected (in the icon-only
sidebar, just the colored dot; hover it for details) and opens Connections.
Settings is the last button in the header.

Camera live video and vendor-specific robot commands are not available yet. OAuth
sign-in is deferred until the app has a registered website redirect; see
[the architecture notes](docs/ARCHITECTURE.md).

## Connect

Open Home and choose **Connect…** (or press Ctrl+L). Enter the address you open
Home Assistant with (for example `http://homeassistant.local:8123`) and a
long-lived access token: in Home Assistant, open your profile, then **Security**,
and create one. The dialog has a link that opens that page.

Home checks the address and token with Home Assistant before it saves anything.
The token goes to your desktop keyring (Secret Service, through `secret-tool`),
never to a settings file. When a token is saved, the dialog says so; leave the
field empty to keep it. If your keyring is locked when Home starts, unlock it and
choose **Try again**. A token saved by the app under its former name, Nixe Home,
is moved to the new name automatically.

Use HTTPS when connecting over an untrusted network. Home Assistant keeps control
of authentication and permissions.

## Devices

Open a tile's details with its **⋯** button, a right-click, or Shift+F10. From
there, **Add to Overview** pins it as a favorite. Arrow keys move between tiles;
Enter or Space uses the tile's main control. Rooms leaves out the diagnostic,
configuration and hidden entities that Home Assistant keeps off its own
dashboards; the eye button in the header (or **Settings → Rooms**) shows them.

Choose the edit button on Overview to reorder or hide its sections. Camera stills
and vacuum maps load only when you ask for them, come through Home Assistant's
authenticated proxy, and are kept in memory, not on disk. Robot buttons appear
only for commands the entity supports. A vacuum or mower is one tile with its
controls and map (a map image or camera on the same device, the current map
first); its many settings entities appear only when showing everything, and its
map and video stay out of the Cameras section. Starting a mower asks for confirmation;
area cleaning uses the areas mapped to the vacuum in Home Assistant.

## Automations

Press **New automation** (Ctrl+N) and choose where it runs:

- **On this computer:** triggers are running it yourself, the desktop starting,
  a schedule, or a Hyprland window opening or closing. Actions send
  notifications, open apps, switch workspaces and set the volume.
- **In Home Assistant:** an entity state change, an optional entity condition,
  and on/off or scene actions. Home Assistant runs it and keeps its traces;
  saving it requires an administrator token.
- **Hybrid:** runs on this computer and reacts to or controls Home Assistant
  devices. It's built on the canvas and never runs while the computer is off,
  so keep home-critical behavior in Home Assistant.

On the canvas, choose a step type and press **Add step**, or drag the button
onto the canvas. Select a step to edit its fields in the inspector, then use
**Connect next step…** (for a question step, **Connect “yes”…** and
**Connect “no”…**) and click the step that follows. Actions can have a timeout,
retries, and an error handler. **Dry run** walks through the steps without doing
anything; **Live test** runs the draft once and shows each step's progress.
Ctrl+S saves; Esc cancels a connection, clears the selection, or closes.

The service runs as you, without root, and only performs these typed actions;
there are no shell commands. Its data is in
`~/.local/share/nyx-home/automation.db`. Check it with
`systemctl --user status nyx-homed`; the app starts it on demand through D-Bus.

`nyx-homectl` manages desktop workflows from a terminal: `list`, `save FILE`,
`run ID`, `enable ID`, `disable ID`, `delete ID`, `history`, `status`, and
`dry-run FILE [CONTEXT]`. An [example workflow](examples/manual-notification.json)
can be imported with `nyx-homectl save examples/manual-notification.json`.

## Keyboard

| Keys | Does |
| --- | --- |
| Ctrl+1 … Ctrl+5 | Overview, Rooms, Automations, Activity, Connections |
| Ctrl+F | Search devices |
| Ctrl+N | New automation |
| Ctrl+L | Connect to Home Assistant |
| Ctrl+B | Collapse the sidebar |
| Ctrl+, | Settings |
| F5 | Refresh the page |
| F1 or ? | All shortcuts (also in Settings → About) |

## Build and install

Requirements: Rust/Cargo, GTK 4.18+, and `secret-tool` from libsecret.

```sh
cargo build --release --locked
./install.sh
```

This installs `nyx-home`, `nyx-homed` and `nyx-homectl` under `~/.local/bin`, a
launcher entry and icon, and the `nyx-homed` user service with D-Bus activation.
An upgrade restarts the service but not an open window. `./uninstall.sh` removes
them; `./uninstall.sh --purge` also removes settings, automations and the token.

## Development

```sh
cargo check --locked
cargo test --locked
cargo run --example check_css   # GTK must accept the stylesheet (needs a display)
```

`NYX_HOME_PAGE=rooms nyx-home` opens a page directly, and `NYX_HOME_OPEN=canvas`
(or `builder`, `new`, `connect`, `settings`, `shortcuts`) opens a dialog at start.
The app uses the pinned Nyx `nyx-core` and `nyx-design` crates. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for boundaries and
[docs/IMPLEMENTATION_PLAN.md](docs/IMPLEMENTATION_PLAN.md) for status and limits.
