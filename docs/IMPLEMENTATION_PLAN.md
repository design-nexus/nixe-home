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

## Next

1. **Phase 3:** Replace the quick template with a When / If / Then builder using
   the same workflow JSON. Add conditions, dry run, workflow validation messages,
   and Home Assistant automation routing only for faithfully representable flows.
2. **Phase 4:** Extend graph validation/runtime to branching and parallel paths;
   add the native node canvas, inspector, undo/redo, and execution highlights.
3. **Phase 5:** Define hybrid workflow ownership, durable event IDs, duplicate
   suppression and offline replay rules before cross-system execution.
4. **Phase 6:** Capability-driven camera, robot and energy views from Home
   Assistant entities.
5. **Phase 7:** Optional assistant integration, accessibility, packaging and
   documentation polish.

## Current limits

- Phase 2 workflows are one trigger followed by a linear action chain.
- Window-close triggers can resolve only windows seen opening while the service
  is connected. Hyprland event triggers need an active user session.
- Interval schedules start anew when the service starts and do not replay missed
  executions. Home-critical automation remains in Home Assistant.
- The advanced canvas, full builder, hybrid workflows, camera previews, and
  robot controls remain in later phases.
