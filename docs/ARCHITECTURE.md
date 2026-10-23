# Nixe Home Architecture

## Phase 0 decisions

- **Repository:** standalone `nixe-home`; suite-wide shared code remains in `nixe-core`
  and `nixe-design`.
- **Application:** one Rust 2024 binary and one application crate for the initial
  frontend. Keep HA transport and credential handling in separate modules. Create
  separate automation crates only when a later phase establishes stable APIs.
- **UI:** GTK4/gtk-rs on the GLib main context. Provider requests run on Tokio worker
  threads and return normalized messages through `async-channel`. Reuse shared Nixe
  theme installation and common keys; use suite semantic colors with sans-serif
  interface text and monospace technical identifiers.
- **Home Assistant:** HA owns device state, areas, scenes, and home-critical
  automations. Nixe uses `/api/states`, service calls, and the authenticated WebSocket
  event API. It does not modify HA's internal storage.
- **Credentials:** the initial manual long-lived access token is written to and read
  from the desktop Secret Service with `secret-tool`; it is never stored in app
  settings, SQLite, command arguments, or logs. A missing or locked service is an
  explicit connection error.
- **Local persistence:** Nixe preferences (connection URL, section, favorites, theme)
  use the shared TOML preferences store. No local SQLite database is needed until
  Nixe-owned workflows or execution history exist.
- **Desktop automation:** later phases will use a user service and D-Bus, without root
  privileges. Home-critical automations remain in Home Assistant. Hybrid workflows
  require explicit ownership and duplicate suppression before they are introduced.

## Authentication constraint

Home Assistant's OAuth client identifier is the application website, and non-default
redirect URIs must be published by that website using a `rel="redirect_uri"` link.
The new repository has no registered website or redirect callback yet. Phase 1
therefore uses a user-created long-lived access token stored in Secret Service. Add
the browser OAuth flow after the Nixe Home website and a native redirect mechanism
are registered; do not send users to a fabricated or unregistered callback.

## Phase 1 data flow

1. The user enters the Home Assistant base URL and token in the GTK connection dialog.
2. Nixe saves the token through Secret Service and keeps the URL in ordinary settings.
3. A Tokio worker fetches entity states over REST, then authenticates to the WebSocket
   API and subscribes to `state_changed` events.
4. Normalized entity updates return to the GTK thread through an async channel.
5. Supported quick controls call HA services with an `entity_id`; HA remains the
   authority for authorization, device capability, and resulting state.

## Current limitations and next architecture decisions

- Area/device registry discovery is best-effort, because Home Assistant may restrict
  registry calls for some users or versions. Entities remain visible if registry
  access is unavailable.
- The quick-control set is intentionally narrow (lights, switches, fans, input
  booleans, and locks). Other domains are visible but do not receive invented controls.
- OAuth redirect registration, dashboard card rearrangement, camera streaming, and
  home and hybrid automation execution are out of scope for this foundation build.

## Phase 2 desktop engine

- `nixe-homed` owns desktop workflow execution and stays running as a systemd user
  service after the window closes. It exposes `Automation1` on the session D-Bus.
- Version 1 workflow JSON is shared by the GUI, CLI and daemon. Node and edge IDs
  are stable. `petgraph` checks the graph before saving or executing. The current
  runtime accepts a single trigger followed by a linear action chain; branching and
  parallel execution will extend the schema and runtime in Phase 4.
- SQLite stores workflows and run records under Nixe Home's XDG data directory.
  Running records are marked interrupted after daemon restart. Intervals do not
  replay missed runs after downtime. Per-workflow concurrent execution is blocked.
- Hyprland socket2 supplies window-open and close events. Event reconnects after
  session changes. The daemon invokes only typed, allowlisted desktop operations
  without a shell; application IDs, numerical ranges and notification lengths are
  validated. External processes have 10-second timeouts. Scripts and imported shell
  commands have no execution path.
- The app queries the service off the GTK thread. The Automations page offers one
  native When / If / Then builder.
  Home Assistant remains authoritative for home-only automation.

## Phase 3 builder and routing

- `Workflow.owner` defaults to `desktop`, preserving Phase 2 JSON. The daemon
  rejects live execution of `home_assistant` workflows. The same node/edge
  representation underlies both owners and the builder.
- The builder validates before save, preserves compatible node IDs on edits, and
  shows a read-only summary for graphs it cannot represent. Dry run evaluates
  supplied Home Assistant state and lists planned or skipped actions without
  invoking services. An unavailable application list yields an unknown condition
  warning rather than a false result.
- The supported Home Assistant subset compiles to a state trigger, optional
  state condition, and on/off or scene actions. The app posts the compiled config
  to HA's validated `/api/config/automation/config/{id}` endpoint, then mirrors
  the Nixe graph in the local service. HA requires administrator access for this
  endpoint. On a local mirror failure after HA accepts the config, the UI reports
  the error; the automation may already exist in HA.
- HA enable/disable and live test resolve the automation entity through its
  state capability `id`, then call HA's `automation.turn_on`, `turn_off`, or
  `trigger` service. Live test passes `skip_condition=false`. HA remains the
  execution owner and retains its own traces; local Activity covers desktop
  runs only.

Home Assistant source references: [automation config endpoint](https://github.com/home-assistant/core/blob/dev/homeassistant/components/config/automation.py),
[config view validation](https://github.com/home-assistant/core/blob/dev/homeassistant/components/config/view.py),
[automation services](https://github.com/home-assistant/core/blob/dev/homeassistant/components/automation/services.py).
