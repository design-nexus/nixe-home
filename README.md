# Home

Nixe Home is a native Linux frontend for an existing Home Assistant instance, with a
separate desktop automation engine planned for later phases. Home Assistant remains
the smart-home backend.

## Current functionality

- Connect to Home Assistant with a long-lived access token.
- Store the credential in the desktop Secret Service (through `secret-tool`), never
  in the settings file.
- Discover entities, areas, and devices; show live state changes and group entities
  by area (with unassigned entities grouped by domain).
- Pin devices to Overview and use basic controls for lights, switches, fans, input
  booleans, and locks.
- Show all six planned sections with clear readiness information for features that
  have not been implemented yet.

The first build does not yet support cameras, robot controls, automation editing, or
desktop actions.
OAuth is deferred until the app has a registered website redirect. See
[the architecture notes](docs/ARCHITECTURE.md).

## Build and install

Requirements: Rust/Cargo, GTK 4.18+, and `secret-tool` from libsecret.

```sh
cargo build --release --locked
./install.sh
```

This installs `nixe-home`, a launcher entry, and the application icon under
`~/.local`. It does not open the app. To run it, use `nixe-home` or select **Home** in
the application launcher.

## Connect

Open **Home** and choose **Connect Home Assistant**. Enter the base URL (for example
`http://homeassistant.local:8123`) and a long-lived access token created in your
Home Assistant profile. Nixe Home stores the token in Secret Service. If your keyring
is locked or unavailable, unlock or enable it before connecting.

Use HTTPS when connecting over an untrusted network. Home Assistant authentication
and service permissions remain managed by Home Assistant.

## Development

```sh
cargo check --locked
cargo test --locked
```

The app uses the pinned Nixe `nixe-core` and `nixe-design` crates. See
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for boundaries and current limitations.
