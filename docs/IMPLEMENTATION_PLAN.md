# Implementation plan

## Completed

- **Phase 0:** Inspected Nixe repositories and conventions; chose a standalone
  repository, pinned shared crates, Home Assistant ownership, Secret Service
  credentials, and a native GTK application. See `ARCHITECTURE.md`.
- **Phase 1:** GTK shell, theme, connection dialog, secure token storage, Home
  Assistant inventory, live state, favorites, and basic device controls.
- **Phase 2:** Independent systemd user daemon, D-Bus API, versioned workflow
  graphs, SQLite workflow and run persistence, interval and Hyprland window
  triggers, typed desktop actions, timeouts, crash recovery, GUI service pages,
  and a CLI for advanced workflow management.
- **Phase 3:** Native When / If / Then builder for desktop and a faithful Home
  Assistant subset, optional condition evaluation, multiple actions, dry run,
  live test, edit, enable/disable, and owner-based execution routing.
- **Phase 4:** Native GTK graph editor with palette drag/drop, pan/zoom, snap,
  auto layout, selection, copy/paste, groups, undo/redo, shortcuts, inspector,
  branching, parallel paths, repeat, event waits, variables, delays, action
  policies, error paths, dry run, live progress, cancellation and Activity detail.
- **Phase 5:** Explicit desktop-owned hybrid workflows, HA event subscriptions,
  typed HA REST actions and state conditions, reconnection, persisted event
  receipts, duplicate suppression, and loop rate limits.
- **Phase 6:** Camera and image still previews, capability-gated vacuum and lawn
  mower controls, mapped vacuum area selection, energy/power sensor views,
  device-health indicators, and reordered/hidden Overview cards.

## Next

1. **Phase 7:** Optional assistant integration, accessibility, packaging and
   documentation polish.

## Current limits

- The simple builder is one trigger, optional simple condition, and a linear
  action chain. Desktop canvas workflows can branch and execute parallel paths.
  The Home Assistant subset supports state changes and basic
  entity on/off or scene actions; its config endpoint requires HA administrator
  access.
- Window-close triggers can resolve only windows seen opening while the service
  is connected. Hyprland event triggers need an active user session.
- Interval schedules start anew when the service starts and do not replay missed
  executions. Home-critical automation remains in Home Assistant.
- Hybrid workflows are desktop-owned and at-most-once for received HA events.
  The HA live WebSocket stream cannot replay events missed while Nixe Home or
  the desktop is offline. A receipt written immediately before a run prevents
  replay after a crash, so a crash at that point can lose the event. Keep
  home-critical automation in HA.
- Camera previews are still images loaded on request. Native HLS/WebRTC playback
  and vendor-specific robot commands are not yet supported. Vacuum maps appear
  only if Home Assistant exposes a map image entity associated with the vacuum.
  The connected HA instance exposed no camera, vacuum, mower, image, energy, or
  power entities during Phase 6 development, so those controls could only be
  checked with mocked responses at that point.
- Home Assistant execution history stays in HA's traces; Activity shows desktop
  and hybrid runs.
