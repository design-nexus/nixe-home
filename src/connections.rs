//! Connections: the Home Assistant server and the desktop automation service.
use crate::{
    app::{Home, Link},
    connection, present, rt, service,
};
use gtk::prelude::*;
use nyx_ui::widgets;
use std::cell::RefCell;

pub struct Page {
    pub root: gtk::Widget,
    pub actions: gtk::Box,
    ha: gtk::Box,
    desktop: gtk::Box,
    shown: RefCell<String>,
}

impl Page {
    pub fn new() -> Self {
        let body = widgets::vbox(0);
        body.add_css_class("settings-page");
        body.add_css_class("home-page");
        body.add_css_class("narrow-page");
        body.append(&widgets::label("HOME ASSISTANT", "group-title"));
        let ha = widgets::vbox(0);
        body.append(&ha);
        body.append(&widgets::label("DESKTOP AUTOMATION SERVICE", "group-title"));
        let desktop = widgets::vbox(0);
        body.append(&desktop);
        let clamp = nyx_ui::clamp::Clamp::new(&body, 760);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .child(&clamp)
            .build();
        let page = Self {
            root: scroll.upcast(),
            actions: widgets::hbox(2),
            ha,
            desktop,
            shown: RefCell::new(String::new()),
        };
        page.show_service(None);
        page
    }

    /// Show the Home Assistant connection; rebuilt only when it changes.
    pub fn refresh(&self, home: &Home) {
        let signature = format!(
            "{:?}|{}|{}|{}|{}|{}|{}",
            home.link,
            home.base,
            home.info.version,
            home.info.location_name,
            home.areas.len(),
            home.device_count,
            home.entities.len()
        );
        if *self.shown.borrow() == signature {
            return;
        }
        *self.shown.borrow_mut() = signature;
        clear(&self.ha);
        let card = widgets::vbox(0);
        card.add_css_class("group-list");
        let tone = match home.link.tone() {
            "success" => "success",
            "danger" => "danger",
            "warning" => "warning",
            _ => "",
        };
        let state = widgets::tag(&home.link.label(), tone);
        card.append(&widgets::row(
            "Status",
            &home
                .link
                .detail()
                .map(|d| gtk::glib::markup_escape_text(d).to_string())
                .unwrap_or_default(),
            Some(state.upcast_ref()),
        ));
        if !home.base.is_empty() {
            let server = widgets::label(&home.base, "mono");
            server.set_selectable(true);
            server.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            card.append(&widgets::row("Server", "", Some(server.upcast_ref())));
        }
        if !home.info.version.is_empty() {
            card.append(&widgets::row(
                "Home Assistant",
                &gtk::glib::markup_escape_text(&home.info.location_name),
                Some(widgets::label(&format!("Version {}", home.info.version), "dim").upcast_ref()),
            ));
        }
        if home.loaded() {
            card.append(&widgets::row(
                "Found",
                "Diagnostic and hidden entities are left out of Rooms unless you ask for them.",
                Some(
                    widgets::label(
                        &format!(
                            "{} · {} · {}",
                            present::plural(home.areas.len(), "area"),
                            present::plural(home.device_count, "device"),
                            present::plural(home.entities.len(), "entity")
                                .replace("entitys", "entities")
                        ),
                        "num",
                    )
                    .upcast_ref(),
                ),
            ));
        }
        card.append(&widgets::row(
            "Access token",
            "Kept in your desktop keyring, never in settings files. Create tokens in your Home Assistant profile under Security.",
            None,
        ));
        self.ha.append(&card);

        let buttons = widgets::hbox(8);
        buttons.add_css_class("card-actions");
        if matches!(home.link, Link::NotSetUp) {
            let connect = gtk::Button::with_label("Connect…");
            connect.add_css_class("suggested-action");
            connect.connect_clicked(|_| connection::dialog());
            buttons.append(&connect);
        } else {
            let replace = gtk::Button::with_label(
                if matches!(home.link, Link::NeedsToken | Link::Rejected) {
                    "Add token…"
                } else {
                    "Change server or token…"
                },
            );
            if matches!(home.link, Link::NeedsToken | Link::Rejected) {
                replace.add_css_class("suggested-action");
            }
            replace.connect_clicked(|_| connection::dialog());
            buttons.append(&replace);
            let again = gtk::Button::with_label("Reconnect");
            again.connect_clicked(|_| connection::reconnect());
            buttons.append(&again);
            if !home.base.is_empty() {
                let base = home.base.clone();
                let open = gtk::Button::with_label("Open in browser");
                open.connect_clicked(move |button| {
                    let window = button.root().and_downcast::<gtk::Window>();
                    gtk::UriLauncher::new(&base).launch(
                        window.as_ref(),
                        None::<&gtk::gio::Cancellable>,
                        |_| {},
                    );
                });
                buttons.append(&open);
            }
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_hexpand(true);
            buttons.append(&spacer);
            buttons.append(&widgets::two_click(
                "Disconnect",
                "Click again to disconnect",
                connection::disconnect,
            ));
        }
        self.ha.append(&buttons);
    }

    pub fn reload_service(&self) {
        rt::spawn(service::status(), |result| {
            if let Some(app) = crate::app::get() {
                app.connections.show_service(Some(result));
            }
        });
    }

    fn show_service(&self, status: Option<Result<String, String>>) {
        clear(&self.desktop);
        let card = widgets::vbox(0);
        card.add_css_class("group-list");
        let (label, tone, detail) = match &status {
            None => ("Checking…", "", String::new()),
            Some(Ok(status)) => ("Running", "success", sentence(status)),
            Some(Err(error)) => (
                "Not running",
                "danger",
                gtk::glib::markup_escape_text(error).to_string(),
            ),
        };
        card.append(&widgets::row(
            "Status",
            &detail,
            Some(widgets::tag(label, tone).upcast_ref()),
        ));
        let hyprland = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some();
        card.append(&widgets::row(
            "Window triggers",
            if hyprland {
                "Hyprland is running, so automations can react to windows opening and closing."
            } else {
                "Window triggers need a Hyprland session."
            },
            Some(
                widgets::tag(
                    if hyprland { "Available" } else { "Unavailable" },
                    if hyprland { "" } else { "warning" },
                )
                .upcast_ref(),
            ),
        ));
        card.append(&widgets::row(
            "Triggers",
            "Run manually, when the desktop starts, on a schedule, when a window opens or closes, or when a Home Assistant entity changes.",
            None,
        ));
        card.append(&widgets::row(
            "Actions",
            "Notifications, opening apps, switching workspaces, setting the volume, and turning Home Assistant lights, switches and scenes on or off.",
            None,
        ));
        card.append(&widgets::row(
            "Safety",
            "Runs as you, without root. Only these typed actions can run; there are no shell commands.",
            None,
        ));
        if matches!(status, Some(Err(_))) {
            let command = widgets::label("systemctl --user enable --now nyx-homed", "mono");
            command.set_selectable(true);
            card.append(&widgets::row(
                "Start it",
                "Run this in a terminal.",
                Some(command.upcast_ref()),
            ));
        }
        self.desktop.append(&card);
        let buttons = widgets::hbox(8);
        buttons.add_css_class("card-actions");
        let again = gtk::Button::with_label("Check again");
        again.connect_clicked(|_| {
            if let Some(app) = crate::app::get() {
                app.connections.reload_service();
            }
        });
        buttons.append(&again);
        self.desktop.append(&buttons);
    }
}

fn sentence(status: &str) -> String {
    let text = status.replace(" · ", ". ").replace("ready", "Ready");
    gtk::glib::markup_escape_text(&format!("{text}.")).to_string()
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
