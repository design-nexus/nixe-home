# Nyx Home Architecture

## Phase 0 decisions

- **Repository:** standalone `nyx-home`; suite-wide shared code remains in `nyx-core`
  and `nyx-design`.
- **Application:** one Rust 2024 binary and one application crate for the initial
  frontend. Keep HA transport and credential handling in separate modules. Create
  separate automation crates only when a later phase establishes stable APIs.
- **UI:** GTK4/gtk-rs on the GLib main context. Provider requests run on Tokio worker
  threads and return normalized messages through `async-channel`. Reuse shared Nyx
  theme installation and common keys; use suite semantic colors with sans-serif
  interface text and monospace technical identifiers.
- **Home Assistant:** HA owns device state, areas, scenes, and home-critical
  automations. Nyx uses `/api/states`, service calls, and the authenticated WebSocket
  event API. It does not modify HA's internal storage.
- **Credentials:** the initial manual long-lived access token is written to and read
  from the desktop Secret Service with `secret-tool`; it is never stored in app
  settings, SQLite, command arguments, or logs. A missing or locked service is an
  explicit connection error.
- **Local persistence:** Nyx preferences (connection URL, section, favorites, theme)
  use the shared TOML preferences store. No local SQLite database is needed until
  Nyx-owned workflows or execution history exist.
- **Desktop automation:** later phases will use a user service and D-Bus, without root
  privileges. Home-critical automations remain in Home Assistant. Hybrid workflows
  require explicit ownership and duplicate suppression before they are introduced.

## Authentication constraint

Home Assistant's OAuth client identifier is the application website, and non-default
redirect URIs must be published by that website using a `rel="redirect_uri"` link.
The new repository has no registered website or redirect callback yet. Phase 1
therefore uses a user-created long-lived access token stored in Secret Service. Add
the browser OAuth flow after the Nyx Home website and a native redirect mechanism
are registered; do not send users to a fabricated or unregistered callback.

## Phase 1 data flow

1. The user enters the Home Assistant base URL and token in the GTK connection dialog.
2. Nyx saves the token through Secret Service and keeps the URL in ordinary settings.
3. A Tokio worker fetches entity states over REST, then authenticates to the WebSocket
   API and subscribes to `state_changed` events.
4. Normalized entity updates return to the GTK thread through an async channel.
5. Supported quick controls call HA services with an `entity_id`; HA remains the
   authority for authorization, device capability, and resulting state.

## Phase 1 limitations at the time

- Area/device registry discovery is best-effort, because Home Assistant may restrict
  registry calls for some users or versions. Entities remain visible if registry
  access is unavailable.
- The quick-control set is intentionally narrow (lights, switches, fans, input
  booleans, and locks). Other domains are visible but do not receive invented controls.
- OAuth redirect registration and camera streaming remain future work.

## Phase 2 desktop engine

- `nyx-homed` owns desktop workflow execution and stays running as a systemd user
  service after the window closes. It exposes `Automation1` on the session D-Bus.
- Version 1 workflow JSON is shared by the GUI, CLI and daemon. Node and edge IDs
  are stable. `petgraph` checks the graph before saving or executing. Desktop
  graphs are rooted trees: every nontrigger node has one incoming edge, and a
  branch must have exactly one `true` and one `false` output. Parallel nodes
  spawn child paths concurrently. A `Stop` node terminates its path. The tree
  restriction avoids ambiguous joins and duplicate action execution.
- SQLite stores workflows and run records under Nyx Home's XDG data directory.
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
  the Nyx graph in the local service. HA requires administrator access for this
  endpoint. On a local mirror failure after HA accepts the config, the UI reports
  the error; the automation may already exist in HA.
- HA enable/disable and live test resolve the automation entity through its
  state capability `id`, then call HA's `automation.turn_on`, `turn_off`, or
  `trigger` service. Live test passes `skip_condition=false`. HA remains the
  execution owner and retains its own traces; local Activity covers desktop
  runs only.

## Phase 4 graph editor

- The GTK DrawingArea renders nodes and Bezier connections with Cairo. Native
  GTK controls provide node selection and parameter editing. Dragged positions
  snap to a 12-pixel grid and persist in optional `positions` metadata; the
  daemon validates positions but never uses them for execution.
- The canvas shares canonical workflow JSON with the simple builder. A graph
  the simple builder cannot represent opens a read-only notice there; canvas
  edits preserve its graph. Dry run walks chosen branches, marks unchosen paths
  skipped, and highlights results on the canvas.
- `BranchAppRunning`, `Parallel`, `Repeat`, event waits, variables, `Delay`, and
  `Stop` run in the desktop daemon. Parallel paths run concurrently. Repeat
  executes its child path at most ten times. Action failure follows a connected
  `error` edge or marks the path failed.
- Canvas metadata supports groups and collapsed views. Shift-click selects
  multiple nodes; a local clipboard copies their internal edges and policies.
  GTK shortcuts cover select, copy/paste, delete and undo/redo. `RunProgress`
  and `CancelRun` expose per-node live status through D-Bus. Activity can fetch
  the same detail after a run. Graphs remain capped at 64 nodes.

Home Assistant source references: [automation config endpoint](https://github.com/home-assistant/core/blob/dev/homeassistant/components/config/automation.py),
[config view validation](https://github.com/home-assistant/core/blob/dev/homeassistant/components/config/view.py),
[automation services](https://github.com/home-assistant/core/blob/dev/homeassistant/components/automation/services.py).

## Phase 4 and 5 execution decisions

- The saved graph stays a rooted tree. A node has one parent; parallel branches
  do not rejoin. This avoids executing a shared descendant twice. Variable
  values flow down a path and fork by value at a parallel or repeat node.
- Retry and timeout are optional policies on typed action nodes. Only actions
  with repeatable effects may retry. An `error` edge can handle a failed action;
  normal edges run only after success. Grouping and positions affect the editor
  only. The daemon validates them but never uses them to choose an action.
- `home_assistant` ownership still compiles only to HA-supported automation
  configuration. `hybrid_desktop` ownership is required when a workflow mixes
  HA entities with desktop triggers or actions. The daemon is the sole executor
  of these hybrid workflows and cannot run while its user session is offline.
- Hybrid HA state triggers use HA's authenticated WebSocket `state_changed`
  subscription. The daemon reads the existing URL from Home settings and the
  credential from Secret Service. Neither goes into workflow JSON or logs.
- A stable HA event identity is derived from the event's context and timestamp
  with its entity ID; SQLite receipts suppress duplicate handling after reconnect.
  The daemon accepts events at most once, marks interrupted runs after a crash,
  and does not replay missed HA events because the live WebSocket API has no
  replay cursor. Future events resume after reconnection. Home-critical behavior
  must stay in HA.
- REST actions use the HA service endpoint and only allow typed entity on/off
  and scene activation. No arbitrary domain/service or shell command is accepted.
  Concurrent execution and rate limits prevent local loops; error edges can
  handle provider failures without hiding the failed node in run history.

References: [Home Assistant WebSocket API](https://contributors.home-assistant.io/core/internals/websocket-api/),
[Home Assistant REST API](https://contributors.home-assistant.io/core/internals/rest-api/).

## Phase 6 device views

- Overview section order and visibility live in ordinary Nyx TOML preferences.
  Favorites remain separate and each section follows Home Assistant's current
  entity state. The Home page keeps area grouping and adds device-specific views.
- Camera stills and image entities are requested only on demand over HA's
  authenticated `camera_proxy` and `image_proxy` endpoints. The bearer token
  stays in memory and requests run off the GTK thread. Responses must have an
  image media type and stay under 8 MiB. GTK textures are kept in memory until
  disconnect. Nyx does not store camera content or bypass HA's privacy state.
- Vacuum and lawn mower controls are gated by each entity's `supported_features`
  bitmask and validated again before the REST service call. Mower start requires
  user confirmation. Vacuum area cleaning passes an area ID from the HA area
  registry to `vacuum.clean_area`; HA remains responsible for configured segment
  mapping and safety. A vacuum map is shown only when an image entity on the
  same device is named as a map.
- Energy and power sensors are shown with their own units. Device health reports
  unavailable entities, low battery sensors, and available updates. No energy
  totals are inferred across mixed units or unrelated meters.

References: [camera proxy](https://github.com/home-assistant/core/blob/dev/homeassistant/components/camera/__init__.py),
[vacuum features](https://github.com/home-assistant/core/blob/dev/homeassistant/components/vacuum/const.py),
[lawn mower features](https://github.com/home-assistant/core/blob/dev/homeassistant/components/lawn_mower/const.py),
[vacuum area cleaning](https://www.home-assistant.io/actions/vacuum.clean_area/).
