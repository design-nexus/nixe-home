//! Settings, as the suite's card over the window.
use crate::{app, connection, prefs};
use gtk::prelude::*;
use nyx_ui::{
    settings_card::{self, Spec},
    widgets,
};

const GROUPS: &[(&str, &str, &str)] = &[
    (
        "Appearance",
        "preferences-desktop-appearance-symbolic",
        "Theme, accent and motion",
    ),
    ("Rooms", "user-home-symbolic", "Which entities are listed"),
    (
        "Home Assistant",
        "network-server-symbolic",
        "Server and access token",
    ),
    (
        "About",
        "help-about-symbolic",
        "Version, shortcuts and your data",
    ),
];

const SPEC: Spec = Spec {
    groups: GROUPS,
    search_text: None,
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

fn page() -> (gtk::Widget, gtk::Box) {
    let body = widgets::vbox(24);
    body.set_margin_top(16);
    body.set_margin_bottom(24);
    body.set_margin_start(20);
    body.set_margin_end(20);
    let p = prefs::get();

    let appearance = group(&body, "Appearance");
    let omarchy = nyx_theme::omarchy_available();
    let following = omarchy && p.mode == nyx_theme::ThemeMode::Omarchy;
    let themes: Vec<(String, String)> = nyx_theme::all()
        .into_iter()
        .map(|t| (t.id, t.name))
        .collect();
    let (theme_row, dropdown) = widgets::choice_row(
        "Theme",
        "Colors for every Nyx app, when they aren't following Omarchy.",
        themes,
        &p.theme,
        |id| {
            prefs::update(|p| {
                p.mode = nyx_theme::ThemeMode::Theme;
                p.theme = id.clone();
            });
            nyx_theme::apply();
        },
    );
    dropdown.set_sensitive(!following);
    if omarchy {
        let dropdown = dropdown.clone();
        let (row, _) = widgets::switch_row(
            "Follow Omarchy theme",
            "Match the colors of the current Omarchy theme as it changes.",
            following,
            move |on| {
                prefs::update(|p| {
                    p.mode = if on {
                        nyx_theme::ThemeMode::Omarchy
                    } else {
                        nyx_theme::ThemeMode::Theme
                    }
                });
                nyx_theme::apply();
                dropdown.set_sensitive(!on);
            },
        );
        appearance.append(&row);
    }
    appearance.append(&theme_row);
    appearance.append(&widgets::accent_row("home"));
    let (motion, _) = widgets::switch_row(
        "Reduce motion",
        "Turn off animations, such as pages fading and toasts sliding in.",
        p.reduce_motion,
        |on| {
            prefs::update(|p| p.reduce_motion = on);
            nyx_theme::apply();
        },
    );
    appearance.append(&motion);
    appearance.append(&widgets::start_row());

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

    let about = group(&body, "About");
    about.append(&widgets::row(
        "Home",
        &format!(
            "Version {}. A native Home Assistant frontend and desktop automation for Nyx.",
            env!("CARGO_PKG_VERSION")
        ),
        None,
    ));
    let (shortcuts, _) = widgets::button_row(
        "Keyboard shortcuts",
        "Every key Home answers to. F1 opens this list too.",
        "Show…",
        |_| {
            settings_card::close();
            app::show_shortcuts();
        },
    );
    about.append(&shortcuts);
    about.append(&widgets::row(
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
