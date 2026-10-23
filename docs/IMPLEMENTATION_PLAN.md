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

## Next

1. **Phase 4:** Extend graph validation/runtime to branching and parallel paths;
   add the native node canvas, inspector, undo/redo, and execution highlights.
2. **Phase 5:** Define hybrid workflow ownership, durable event IDs, duplicate
   suppression and offline replay rules before cross-system execution.
3. **Phase 6:** Capability-driven camera, robot and energy views from Home
   Assistant entities.
4. **Phase 7:** Optional assistant integration, accessibility, packaging and
   documentation polish.

## Current limits

- Current workflows are one trigger, optional simple condition, and a linear
  action chain. The Home Assistant subset supports state changes and basic
  entity on/off or scene actions; its config endpoint requires HA administrator
  access.
- Window-close triggers can resolve only windows seen opening while the service
  is connected. Hyprland event triggers need an active user session.
- Interval schedules start anew when the service starts and do not replay missed
  executions. Home-critical automation remains in Home Assistant.
- The advanced canvas, hybrid workflows, camera previews, and
  robot controls remain in later phases. Home Assistant execution history stays
  in HA's traces; the Activity page currently shows desktop runs.
