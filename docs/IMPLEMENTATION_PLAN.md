# Implementation plan

## Completed

- **Phase 0:** Inspected Nyx repositories and conventions; chose a standalone
  repository, pinned shared crates, Home Assistant ownership, Secret Service
  credentials, and a native GTK application. See `ARCHITECTURE.md`.
- **Phase 1:** GTK shell, theme, connection dialog, secure token storage, Home
  Assistant inventory, live state, favorites, and basic device controls.
- **Phase 2:** Independent systemd user daemon, D-Bus API, versioned workflow
  graphs, SQLite workflow and run persistence, interval and Hyprland window
  triggers, typed desktop actions, timeouts, crash recovery, and a CLI for
  advanced workflow management.
- **Phase 3:** Native When / If / Then builder for desktop and a faithful Home
  Assistant subset, optional condition evaluation, multiple actions, dry run,
  live test, edit, enable/disable, and owner-based execution routing.
- **Phase 4:** Native GTK graph editor with drag-and-drop step placement,
  pan/zoom, snap, auto layout, selection, copy/paste, groups, undo/redo,
  shortcuts, inspector, branching, parallel paths, repeat, event waits,
  variables, delays, action policies, error paths, dry run, live progress,
  cancellation and Activity detail.
- **Phase 5:** Explicit desktop-owned hybrid workflows, HA event subscriptions,
  typed HA REST actions and state conditions, reconnection, persisted event
  receipts, duplicate suppression, and loop rate limits.
- **Phase 6:** Camera and image still previews, capability-gated vacuum and lawn
  mower controls, mapped vacuum area selection, energy/power sensor views,
  device-health indicators, and reordered/hidden Overview sections.
- **Phase 7 (UI, fixes, accessibility, packaging):** see below. Assistant
  integration was left for later.

## Phase 7: what changed

A review found that several earlier phases were marked complete but did not
work as described. These are fixed:

- **Desktop runs over D-Bus panicked.** `zbus` ran its handlers on its own
  async-io executor, where `tokio::spawn` has no runtime. Run, Live test and
  `nyx-homectl run` panicked and left the service unable to answer any later
  call. `zbus` now runs on Tokio.
- **Hyprland window triggers never fired.** The daemon read the workspace field
  of `openwindow` events instead of the window class.
- **Event receipts were written before the rate-limit and already-running
  checks,** so a refused event was lost. Receipts are now written last.
- **The loop limit only counted Home Assistant runs, globally.** It now applies
  per workflow to every automatic trigger (20 runs a minute).
- **Every Home Assistant event re-read and parsed every workflow from SQLite.**
  Workflows are cached and the cache is cleared on save, delete or enable.
- **The hybrid link ran `secret-tool` every 10 seconds;** it now re-reads only
  the saved URL, and the token after a reconnect.
- **Live updates dropped the entity's device,** so a vacuum map vanished after
  its first state change.
- **The rename from Nixe to Nyx changed the keyring lookup** without moving the
  saved token. Home now moves a `nixe-home` token to `nyx-home` on first load.
- **The canvas could panic** when a pending connection's source step was
  deleted or undone; undo didn't restore the owner; Ctrl+Y redid while typing;
  progress polling continued after the window closed.
- **Manual runs and live tests refused disabled workflows.** Only automatic
  triggers require a workflow to be enabled now.

The interface moved to Nyx Design 2.0:

- Unified header with the suite's Settings button last, a collapsible sidebar
  (Ctrl+B, icon-only below 980 px) with the Home Assistant connection at its
  foot, toasts, and the suite's settings card. There is no status bar; keyboard
  shortcuts are on F1 and in Settings → About.
- **Overview:** useful summary cards (lights on, open doors and windows, people
  home, indoor temperature, device health), favorite devices, a link to the
  cameras, and robot, energy and health sections shown only when there is data.
  Sections are reordered and hidden in place. Favorites saved as entities move
  to their devices.
- **Rooms** (formerly "Home"): one card per device in sections by type on a
  unit grid (cards 1, 2 or 4 units each way; sections side by side), with
  colored scene tiles, room chips and search. Arrange mode drags, moves and
  resizes cards and reorders sections; the layout is saved. Light group cards for
  Home Assistant groups and for each room's lights; groups are made, changed and
  deleted from Home. Devices can be renamed, moved to another area, and hidden
  in Nyx Home; a group can hide its lights. Cards update in place and show what matters for
  each kind: thermostat readout, mode, setpoints and fan; light brightness and
  color swatch; camera still and indicators; robot commands and a map button;
  media, purifier, security, lock, cover, switch and sensor cards. Each kind has
  its own color from the theme. A details popup holds every other entity of the
  device. Diagnostic and hidden entities are left out unless asked for.
- **Cameras:** every camera's last still by room, with the security mode. Stills
  never refresh on their own; a viewer window zooms, pans and refreshes them.
- **Automations:** one New automation chooser that explains where each kind
  runs; rows with an enabled switch and a menu (edit, canvas, run, delete).
- **Activity:** automation names, status with icon and text, relative times,
  steps by name, an All / Failed filter, and refresh while visible.
- **Connections** (formerly System and Integrations): the Home Assistant server,
  version and token, and the desktop service's state.
- **Connecting:** the dialog checks the address and token with Home Assistant
  before saving anything, says when a token is already saved, and links to
  where tokens are created. Keyring access no longer blocks the window, and a
  locked keyring can be retried.
- **Builder and canvas:** number fields, readable dry runs, labeled fields per
  step type, explicit yes/no branch outputs, a connection banner, zoom at the
  pointer, Fit view, Ctrl+S, and a prompt about unsaved changes. Validation
  errors name the step they're about.

Accessibility: icon buttons and switches have accessible names, form labels
are linked to their fields, status is never shown by color alone, Reduce motion
turns animations off, cards work from the keyboard (arrows, Enter or Space,
Shift+F10 for details), the canvas can be walked with Tab and moved with the
arrow keys, and F1 lists every shortcut.

Packaging: the service file and a new D-Bus activation file use the install
path, the service is `Type=dbus`, the installer restarts the daemon on upgrade
and copes with no user bus, and `uninstall.sh` removes everything (`--purge`
also removes settings, automations and the token).

## Next

1. Optional assistant integration.
2. WebRTC (go2rtc) for lower-latency live video; HLS is used now.

## Current limits

- The simple builder is one trigger, an optional condition, and a linear
  action chain. Canvas workflows can branch and run parallel paths. The Home
  Assistant subset supports state changes and basic entity on/off or scene
  actions; its config endpoint requires HA administrator access.
- Window-close triggers can resolve only windows seen opening while the service
  is connected. Hyprland triggers need an active user session.
- Interval schedules start anew when the service starts and do not replay missed
  executions. Home-critical automation belongs in Home Assistant.
- Hybrid workflows are desktop-owned and at-most-once for received HA events.
  The HA WebSocket stream cannot replay events missed while the desktop is
  offline. A receipt is written just before a run starts, so a crash at that
  moment can lose the event.
- Camera previews are Home Assistant's last stills, loaded once per session.
  Robot maps appear only if Home Assistant exposes a map image or camera entity
  on the robot's device.
- Home Assistant execution history stays in HA's traces; Activity shows desktop
  and hybrid runs.
- The canvas draws with Cairo; its steps are reachable by keyboard and
  summarized for screen readers, but individual steps are not separate
  accessible objects.
