# Home

Nyx Home is a native Linux frontend for an existing Home Assistant instance, with a
separate desktop automation engine. Home Assistant remains the smart-home backend.

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/design-nexus/nyx-home/main/install.sh | bash
```

## What it does

- **Overview:** lights on, open doors and windows, who's home, the indoor
  temperature and device health at a glance, your favorite devices, a link to
  your cameras, robots, and energy sensors when Home Assistant has them.
- **Rooms:** one card per device on a grid, with scenes as colored tiles and
  search. All shows every room in sections by type (Lights, Climate, Cameras,
  …) that sit side by side; a room's chip shows just that room's cards on one
  grid. Each card shows what's useful for that kind of device; everything else
  is in its details.
- **Cameras:** every camera with its last picture, each card saying its room.
- **Automations:** desktop automations that run in a user service even with the
  window closed, Home Assistant automations that run on your server, and hybrid
  automations that connect the two. Build them with a simple When / Only if /
  Then builder or on a canvas with branches, parallel paths, waits and retries.
- **Activity:** what desktop and hybrid automations did, step by step.
- **Connections:** the Home Assistant server and token, and the desktop service.

The foot of the sidebar shows whether Home is connected (in the icon-only
sidebar, just the colored dot; hover it for details) and opens Connections.
Settings is the last button in the header.

OAuth sign-in is deferred until the app has a registered website redirect; see
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

Home Assistant's entities are gathered into one card per device, each in its
own color:

- **Thermostat:** the current temperature and what it's doing, the mode, the
  target (or heat-to and cool-to in heat/cool mode), and the fan setting.
- **Light:** on/off, a brightness slider, and a color button with a swatch of
  colors and whites.
- **Light group:** one card for several lights: one switch, one brightness
  and one color for all of them. Groups made in Home Assistant come first, and
  every room with two or more lights gets one too.
- **Camera:** its last picture, enabled switch, motion, person, live and
  battery indicators.
- **Vacuum and mower:** state and battery, icon buttons for start, pause and
  dock, a menu with the other commands, and a map button that opens the map.
- **Media, purifier and fan, security, lock, cover, switches and sensors:** the
  controls and readings that matter for each.

Open a card's details with its **⋯** button, a right-click, or Shift+F10: every
other control, reading, setting and diagnostic of the device, a search when
there are many, **Add to Overview** to pin it, and a link to the device in Home
Assistant. Its **Device** section renames it, moves it to another room (Home
Assistant calls rooms *areas*), and hides it in Nyx Home; hidden devices stay
in Home Assistant, show again with the eye button, and are listed in
**Settings → Rooms** to unhide. Arrow keys move between cards; Enter or Space
uses the card's main control. Rooms leaves out diagnostic, configuration and
hidden entities; the eye button in the header (or **Settings → Rooms**) shows
them.

Tap a scene tile to run it. Its **⋯** button (or a right-click, a long press,
or Shift+F10) renames it, puts it in a room, or hides it in Nyx Home; scenes
and scripts made in Home Assistant also open in its editor, and ones from
another app (SmartThings, Hue, …) say where to change what they do.

To make a light group, use **+** on the Lights heading, or a room's lights
card and **Make group…**: choose a name, a room and the lights. Home Assistant
keeps the group (so it works in its app, automations and voice assistants too)
and can hide the grouped lights there. A group's details change its lights or
delete it. **Hide its lights**, in a group's or a room's lights details, hides
the single lights in Nyx Home only, so just the group shows; they stay in the
group's details. Making, changing and moving things in Home Assistant needs an
administrator token.

Cards sit on a grid and come in a few sizes, in rows by columns: a toggle,
lock or single sensor is 1 × 4; a light, light group, purifier, media player,
robot or security panel is 2 × 4; a thermostat or camera is 4 × 4; a scene is
1 × 2. A thermostat keeps its layout when it's off, with its setpoints dimmed.

The grid button in the header of Rooms and Cameras starts **Arrange** mode
(on Overview, the edit button does): drag a card by anywhere on it to move it
within its section, or focus it and use Ctrl+arrow keys; its size button (or
Ctrl+S) offers the sizes that card comes in; the arrows on a section heading
move the section. Controls are paused while arranging. **Done** (the grid
button again) or Esc finishes; **Reset** puts the page back as it was. The
order and sizes are saved; each room's tab and the Cameras page keep their own
order.

A robot's card opens its own window (the map button, or Enter on the card),
like the vendor apps: the live map on the left and a panel on the right with its
state, battery and this session's progress, icon commands, the settings that
matter (cleaning mode, suction, route, carpets; or mowing mode, height,
efficiency, direction, rain protection and live position), station or mower
actions, and **All settings…**. The floor plan refreshes every 10 seconds while
the robot works. A mower reports where it is while it mows only with **Live
position** on. Home Assistant's robot integration draws the floor plan; Home
draws what you read and touch on top of it, sharp at any zoom: room or zone names, the robot, its dock, no-go zones and
walls, and the mower's trail. Tap a room's name to clean just that room (Home
asks first), or a zone's name to mow it (after the mower safety check). Turn off
the integration's own room names, icons, robot and dock in its map options so
they don't show twice.

Cameras that Home Assistant can stream have a **Live** button (or L) in their
viewer: the video plays muted (M for sound), and after three minutes Home asks
"Still watching?" and stops unless you choose **Keep watching**, to save battery
cameras. Closing the viewer stops it, and a fresh still replaces the old one.
One camera plays at a time. Live video uses mpv's library (libmpv). The Live
dot turns amber while the camera wakes and red once video plays. A camera
marked **Plugged in** (in its details; kept as a "Plugged in" label in Home
Assistant) never stops on its own, and its card shows power instead of a
battery level.

Some integrations give Home Assistant no stream of their own; the eufy-sdk
bridge is one. Home then streams a stand-in: a Generic Camera on the bridge's
RTSP stream (`rtsp://<home-assistant>:8554/<camera serial>`), named like the
camera with `_live` added (`camera.driveway_live`) and hidden in Home
Assistant. The camera's own RTSP setting can stay off.

Choose the edit button on Overview to reorder or hide its sections. Camera stills
and maps come from Home Assistant's last snapshot through its authenticated
proxy, load once per session, and never refresh on their own, so battery
cameras aren't woken. Click one to open it larger: zoom with the buttons,
Ctrl+scroll, a pinch or + and −, drag to pan, Refresh for a new one, and Esc or
Close to put it away. Pictures are kept in memory, not on disk. Robot buttons
appear only for commands the entity supports. Starting a mower asks for
confirmation; area cleaning uses the areas mapped to the vacuum in Home
Assistant.

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
| Ctrl+1 … Ctrl+6 | Overview, Rooms, Cameras, Automations, Activity, Connections |
| Ctrl+F | Search devices |
| Ctrl+N | New automation |
| Ctrl+L | Connect to Home Assistant |
| Ctrl+B | Collapse the sidebar |
| Ctrl+, | Settings |
| F5 | Refresh the page |
| F1 or ? | All shortcuts (also in Settings → About) |

## Build and install

Requirements: Rust/Cargo, GTK 4.18+, `secret-tool` from libsecret, and libmpv
(the `mpv` package) for live camera video.

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
