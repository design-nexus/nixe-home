//! Settings, as the suite's card over the window.
use crate::{app, connection, prefs};
use gtk::prelude::*;
use nyx_ui::{
    settings_card::{self, About, Spec, Standard},
    widgets,
};

const GROUPS: &[(&str, &str, &str)] = &[
    ("Rooms", "user-home-symbolic", "Which devices are listed"),
    ("Cameras", "camera-video-symbolic", "Live video viewing timeout"),
    (
        "Home Assistant",
        "network-server-symbolic",
        "Server, access token, your data",
    ),
];

const SPEC: Spec = Spec {
    groups: GROUPS,
    search_text: None,
    standard: Some(Standard {
        app: "home",
        about: About {
            name: "Home",
            icon: "io.github.design_nexus.nyx.Home",
            version: env!("CARGO_PKG_VERSION"),
            description: "Home Assistant and desktop automation.",
            licence: "MIT",
            website: "https://github.com/design-nexus/nyx-home",
            issues: "https://github.com/design-nexus/nyx-home/issues",
            credits: &[("Home Assistant", "your devices")],
        },
        shortcuts: app::SHORTCUTS,
        reduce_motion: || prefs::get().reduce_motion,
        set_reduce_motion: |on| prefs::update(|p| p.reduce_motion = on),
    }),
};

pub fn open(overlay: &gtk::Overlay, window: &gtk::ApplicationWindow) {
    let (root, body) = page();
    settings_card::open_page(overlay, window, SPEC, &root, &body);
}

fn group(body: &gtk::Box, title: &str) -> gtk::Box {
    let wrap = widgets::vbox(10);
    wrap.set_widget_name(title);
    wrap.add_css_class("settings-group");
    wrap.append(&widgets::label(&title.to_uppercase(), "group-title"));
    let card = widgets::vbox(0);
    card.add_css_class("group-list");
    wrap.append(&card);
    body.append(&wrap);
    card
}

/// Devices hidden in Nyx Home, each with Unhide.
fn hidden_devices(rooms: &gtk::Box) {
    let hidden = prefs::get().hidden_devices;
    let names: Vec<(String, String)> = app::get()
        .map(|app| {
            let devices = app.home.borrow().device_list(true);
            hidden
                .iter()
                .map(|id| {
                    let name = devices
                        .iter()
                        .find(|d| d.id == *id)
                        .map(|d| d.name.clone())
                        .unwrap_or_else(|| "A device no longer in Home Assistant".into());
                    (id.clone(), name)
                })
                .collect()
        })
        .unwrap_or_default();
    if names.is_empty() {
        rooms.append(&widgets::row(
            "Hidden devices",
            "None. Hide a device from its details (its ⋯ button).",
            None,
        ));
        return;
    }
    for (id, name) in names {
        let unhide = gtk::Button::with_label("Unhide");
        unhide.set_valign(gtk::Align::Center);
        unhide.update_property(&[gtk::accessible::Property::Label(&format!("Unhide {name}"))]);
        let row = widgets::row(&name, "Hidden in Nyx Home", Some(unhide.upcast_ref()));
        let r = row.clone();
        unhide.connect_clicked(move |_| {
            crate::details::set_hidden(&id, false);
            r.set_visible(false);
        });
        rooms.append(&row);
    }
}

fn page() -> (gtk::Widget, gtk::Box) {
    let body = widgets::vbox(24);
    body.set_margin_top(16);
    body.set_margin_bottom(24);
    body.set_margin_start(20);
    body.set_margin_end(20);
    let p = prefs::get();

    let rooms = group(&body, "Rooms");
    let (all, _) = widgets::switch_row(
        "Show diagnostic and hidden entities",
        "List the signal-strength, configuration and hidden entities that Home Assistant keeps out of its own dashboards.",
        p.show_all_entities,
        |on| {
            prefs::update(|p| p.show_all_entities = on);
            app::rebuild();
        },
    );
    rooms.append(&all);
    let (overview, _) = widgets::button_row(
        "Overview sections",
        "Reorder or hide the sections on Overview.",
        "Customize…",
        |_| {
            settings_card::close();
            app::show(app::Section::Overview);
            if let Some(app) = app::get() {
                app.overview.set_editing(true);
            }
        },
    );
    rooms.append(&overview);
    hidden_devices(&rooms);

    let cameras = group(&body, "Cameras");
    let (row, _) = widgets::choice_row("Ask if still watching after", "For battery cameras. Takes effect when live video next starts.",
        widgets::opts(&[("180", "3 minutes"), ("600", "10 minutes"), ("1800", "30 minutes"), ("0", "Never")]),
        &prefs::get().camera_ask_seconds.to_string(), |id| prefs::update(|p| p.camera_ask_seconds = id.parse().unwrap_or(180)));
    cameras.append(&row);
    let ha = group(&body, "Home Assistant");
    let base = app::get()
        .map(|a| a.home.borrow().base.clone())
        .unwrap_or_default();
    let (server, _) = widgets::button_row(
        "Server and token",
        &if base.is_empty() {
            "Not connected.".to_string()
        } else {
            gtk::glib::markup_escape_text(&base).to_string()
        },
        if base.is_empty() {
            "Connect…"
        } else {
            "Change…"
        },
        |_| {
            settings_card::close();
            connection::dialog();
        },
    );
    ha.append(&server);

    ha.append(&widgets::row(
        "Your data",
        "Settings are in <tt>~/.config/nyx-home</tt> and automations in <tt>~/.local/share/nyx-home</tt>. The Home Assistant token stays in your keyring.",
        None,
    ));

    let root = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .vexpand(true)
        .child(&body)
        .build();
    (root.upcast(), body)
}
