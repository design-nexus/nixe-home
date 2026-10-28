//! Overview: a summary of the home, favorite devices, and the cameras,
//! robots, energy and device health that need a glance.
use crate::{
    app::{self, Home, Link},
    cards, connection,
    device::{Device, Kind},
    grid::{self, CardGrid},
    home_features, prefs, present,
};
use gtk::prelude::*;
use nyx_ui::widgets;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap},
};

pub struct Page {
    pub root: gtk::Widget,
    pub actions: gtk::Box,
    stack: gtk::Stack,
    empty: gtk::Box,
    body: gtk::Box,
    customize: gtk::ToggleButton,
    editing: Cell<bool>,
    summary: RefCell<Option<gtk::FlowBox>>,
    lists: RefCell<HashMap<&'static str, gtk::Box>>,
    shown_link: RefCell<Option<Link>>,
}

impl Page {
    pub fn new() -> Self {
        let stack = gtk::Stack::new();
        let empty = widgets::vbox(0);
        empty.set_vexpand(true);
        stack.add_named(&empty, Some("empty"));
        let body = widgets::vbox(0);
        body.add_css_class("settings-page");
        body.add_css_class("home-page");
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .child(&body)
            .build();
        stack.add_named(&scroll, Some("content"));
        let actions = widgets::hbox(2);
        let customize = gtk::ToggleButton::new();
        customize.set_icon_name("document-edit-symbolic");
        customize.add_css_class("bar-button");
        customize.set_valign(gtk::Align::Center);
        customize.set_tooltip_text(Some("Customize Overview"));
        customize.update_property(&[gtk::accessible::Property::Label("Customize Overview")]);
        customize.connect_toggled(|button| {
            if let Some(app) = app::get() {
                app.overview.set_editing(button.is_active());
            }
        });
        actions.append(&customize);
        Self {
            root: stack.clone().upcast(),
            actions,
            stack,
            empty,
            body,
            customize,
            editing: Cell::new(false),
            summary: RefCell::new(None),
            lists: RefCell::new(HashMap::new()),
            shown_link: RefCell::new(None),
        }
    }

    pub fn editing(&self) -> bool {
        self.editing.get()
    }

    pub fn set_editing(&self, on: bool) {
        if self.editing.replace(on) == on {
            return;
        }
        if self.customize.is_active() != on {
            self.customize.set_active(on);
        }
        app::rebuild();
    }

    pub fn refresh(&self, home: &Home, changed: Option<&BTreeSet<String>>) {
        if !home.loaded() {
            if self.shown_link.borrow().as_ref() != Some(&home.link) {
                set_empty(&self.empty, &home.link);
                *self.shown_link.borrow_mut() = Some(home.link.clone());
            }
            self.stack.set_visible_child_name("empty");
            self.customize.set_sensitive(false);
            return;
        }
        *self.shown_link.borrow_mut() = None;
        self.customize.set_sensitive(true);
        self.stack.set_visible_child_name("content");
        if changed.is_some() {
            // Cards keep themselves current; the summary and lists are refilled.
            if let Some(flow) = self.summary.borrow().as_ref() {
                fill_summary(flow, home);
            }
            for (id, list) in self.lists.borrow().iter() {
                fill_list(id, list, home);
            }
            return;
        }
        self.build(home);
    }

    fn build(&self, home: &Home) {
        clear(&self.body);
        self.lists.borrow_mut().clear();
        migrate_favorites(home);
        let devices = home.device_list(false);
        *self.summary.borrow_mut() = None;
        let settings = prefs::get();
        let editing = self.editing.get();
        let order = section_order(&settings);
        if editing {
            let note = widgets::label(
                "Reorder sections with the arrows and turn off the ones you don't need. Drag cards to move them and use their size button to resize them. Press Esc or the edit button when you're done.",
                "group-note",
            );
            note.set_wrap(true);
            note.set_margin_top(4);
            self.body.append(&note);
        }
        for (index, id) in order.iter().enumerate() {
            let hidden = settings.dashboard_hidden.contains(id);
            if hidden && !editing {
                continue;
            }
            let content = match id.as_str() {
                "summary" => {
                    let flow = summary_grid();
                    fill_summary(&flow, home);
                    *self.summary.borrow_mut() = Some(flow.clone());
                    Some(flow.upcast::<gtk::Widget>())
                }
                "favorites" => Some(favorites(home, &prefs::get().favorite_devices, editing)),
                "cameras" => cameras_link(&devices),
                "robots" => grid_of(
                    home,
                    "overview:robots",
                    devices.iter().filter(|d| d.kind == Kind::Robot),
                    editing,
                ),
                "energy" | "health" => {
                    let list = widgets::vbox(0);
                    list.add_css_class("group-list");
                    let id: &'static str = if id == "energy" { "energy" } else { "health" };
                    fill_list(id, &list, home);
                    let empty = list.first_child().is_none();
                    self.lists.borrow_mut().insert(id, list.clone());
                    (!empty).then(|| list.upcast())
                }
                _ => None,
            };
            if content.is_none() && !editing {
                continue;
            }
            self.body
                .append(&section_head(id, index, order.len(), hidden, editing));
            match content {
                Some(widget) => {
                    if hidden {
                        widget.add_css_class("dimmed");
                    }
                    self.body.append(&widget);
                }
                None => {
                    let note = widgets::label("Nothing to show yet.", "group-note");
                    self.body.append(&note);
                }
            }
        }
    }
}

/// Cards for `devices` on the card grid, in their saved order.
fn grid_of<'a>(
    home: &Home,
    key: &str,
    devices: impl Iterator<Item = &'a Device>,
    editing: bool,
) -> Option<gtk::Widget> {
    let list: Vec<&Device> = devices.collect();
    if list.is_empty() {
        return None;
    }
    let g = CardGrid::new(
        key,
        cards::maker(list.iter().map(|d| (*d).clone()).collect(), true),
    );
    let ids: Vec<String> = list.iter().map(|d| d.id.clone()).collect();
    let ids = if key == "overview:favorites" {
        ids
    } else {
        let saved = prefs::get()
            .card_order
            .get(key)
            .cloned()
            .unwrap_or_default();
        grid::ordered(&saved, ids)
    };
    for id in ids {
        if let Some(d) = list.iter().find(|d| d.id == id) {
            cards::add_to(&g, &id, cards::size_options(d, home));
        }
    }
    g.set_arranging(editing);
    Some(g.upcast())
}

/// Pinned devices; a light its group hides still shows here.
fn favorites(home: &Home, favorites: &[String], editing: bool) -> gtk::Widget {
    let mut devices = home.device_list(true);
    devices.retain(|d| !d.hidden);
    let pinned: Vec<&Device> = favorites
        .iter()
        .filter_map(|id| devices.iter().find(|d| d.id == *id))
        .collect();
    if let Some(grid) = grid_of(home, "overview:favorites", pinned.into_iter(), editing) {
        return grid;
    }
    let row = widgets::hbox(12);
    row.add_css_class("card");
    row.add_css_class("hint-card");
    let text = widgets::label(
        "Pin the devices you use most. Open a device's details in Rooms and choose Add to Overview.",
        "dim",
    );
    text.set_wrap(true);
    text.set_hexpand(true);
    row.append(&text);
    let open = gtk::Button::with_label("Open Rooms");
    open.set_valign(gtk::Align::Center);
    open.connect_clicked(|_| app::show(app::Section::Rooms));
    row.append(&open);
    row.upcast()
}

/// A card that leads to the Cameras page, with how many there are.
fn cameras_link(devices: &[Device]) -> Option<gtk::Widget> {
    let count = devices.iter().filter(|d| d.kind == Kind::Camera).count();
    if count == 0 {
        return None;
    }
    let row = widgets::hbox(12);
    row.add_css_class("card");
    row.add_css_class("hint-card");
    row.add_css_class("kind-camera");
    let chip = gtk::Image::from_icon_name("camera-web-symbolic");
    chip.add_css_class("chip");
    row.append(&chip);
    let text = widgets::vbox(2);
    text.set_hexpand(true);
    text.append(&widgets::label(
        &present::plural(count, "camera"),
        "card-name",
    ));
    text.append(&widgets::label("Their last pictures, by room", "card-sub"));
    row.append(&text);
    let open = gtk::Button::with_label("Open Cameras");
    open.set_valign(gtk::Align::Center);
    open.connect_clicked(|_| app::show(app::Section::Cameras));
    row.append(&open);
    Some(row.upcast())
}

/// Favorites used to be entities; pin their devices instead, once.
fn migrate_favorites(home: &Home) {
    let old = prefs::get().favorite_entities;
    if old.is_empty() || home.entities.is_empty() {
        return;
    }
    let ids: Vec<String> = old
        .iter()
        .filter_map(|e| home.entities.get(e))
        .map(|e| {
            e.device_id
                .clone()
                .unwrap_or_else(|| format!("entity:{}", e.entity_id))
        })
        .collect();
    prefs::update(|p| {
        for id in ids {
            if !p.favorite_devices.contains(&id) {
                p.favorite_devices.push(id);
            }
        }
        p.favorite_entities.clear();
    });
}

fn clear(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn section_order(settings: &prefs::Prefs) -> Vec<String> {
    let mut order: Vec<String> = settings
        .dashboard_order
        .iter()
        .filter(|id| {
            home_features::DASHBOARD_CARDS
                .iter()
                .any(|(card, _)| card == id)
        })
        .cloned()
        .collect();
    for (id, _) in home_features::DASHBOARD_CARDS {
        if !order.iter().any(|saved| saved == id) {
            order.push(id.into());
        }
    }
    order
}

fn section_title(id: &str) -> &'static str {
    home_features::DASHBOARD_CARDS
        .iter()
        .find(|(card, _)| *card == id)
        .map_or("Section", |(_, name)| name)
}

fn section_head(id: &str, index: usize, count: usize, hidden: bool, editing: bool) -> gtk::Box {
    let head = widgets::hbox(4);
    head.add_css_class("group-head");
    let title = widgets::label(&section_title(id).to_uppercase(), "group-title");
    title.set_hexpand(true);
    head.append(&title);
    if !editing {
        return head;
    }
    let name = section_title(id);
    for (icon, tip, step, enabled) in [
        ("go-up-symbolic", "Move up", -1isize, index > 0),
        ("go-down-symbolic", "Move down", 1, index + 1 < count),
    ] {
        let button = widgets::icon_button(icon, &format!("{tip}: {name}"));
        button.set_sensitive(enabled);
        let id = id.to_string();
        button.connect_clicked(move |_| {
            prefs::update(|p| {
                let mut order = section_order(p);
                if let Some(at) = order.iter().position(|o| *o == id) {
                    let to = (at as isize + step).clamp(0, order.len() as isize - 1) as usize;
                    order.swap(at, to);
                }
                p.dashboard_order = order;
            });
            app::rebuild();
        });
        head.append(&button);
    }
    let show = gtk::Switch::new();
    show.set_active(!hidden);
    show.set_valign(gtk::Align::Center);
    show.set_tooltip_text(Some(&format!("Show {name} on Overview")));
    show.update_property(&[gtk::accessible::Property::Label(&format!("Show {name}"))]);
    let id = id.to_string();
    show.connect_active_notify(move |switch| {
        let visible = switch.is_active();
        prefs::update(|p| {
            p.dashboard_hidden.retain(|h| *h != id);
            if !visible {
                p.dashboard_hidden.push(id.clone());
            }
        });
        app::rebuild();
    });
    head.append(&show);
    head
}

fn summary_grid() -> gtk::FlowBox {
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_min_children_per_line(2);
    flow.set_max_children_per_line(5);
    flow.set_column_spacing(12);
    flow.set_row_spacing(12);
    flow.set_valign(gtk::Align::Start);
    flow
}

fn fill_summary(flow: &gtk::FlowBox, home: &Home) {
    while let Some(child) = flow.first_child() {
        flow.remove(&child);
    }
    let s = present::summarize(home.entities.values());
    let mut stats = vec![(
        "display-brightness-symbolic",
        if s.lights == 0 {
            "–".to_string()
        } else {
            format!("{} of {}", s.lights_on, s.lights)
        },
        "Lights on",
        "",
    )];
    stats.push((
        "user-home-symbolic",
        s.open.to_string(),
        if s.open == 1 {
            "Open door or window"
        } else {
            "Open doors and windows"
        },
        if s.open > 0 { "warning" } else { "" },
    ));
    if s.people > 0 {
        stats.push((
            "avatar-default-symbolic",
            format!("{} of {}", s.people_home, s.people),
            "People home",
            "",
        ));
    }
    if let Some(temperature) = s.climate {
        stats.push(("weather-few-clouds-symbolic", temperature, "Indoors", ""));
    }
    stats.push((
        if s.issues > 0 {
            "dialog-warning-symbolic"
        } else {
            "emblem-ok-symbolic"
        },
        if s.issues > 0 {
            s.issues.to_string()
        } else {
            "All good".into()
        },
        if s.issues == 1 {
            "Needs attention"
        } else if s.issues > 0 {
            "Need attention"
        } else {
            "Device health"
        },
        if s.issues > 0 { "warning" } else { "success" },
    ));
    for (icon, value, label, tone) in stats {
        let card = widgets::vbox(4);
        card.add_css_class("card");
        card.add_css_class("stat");
        if !tone.is_empty() {
            card.add_css_class(tone);
        }
        let image = gtk::Image::from_icon_name(icon);
        image.set_halign(gtk::Align::Start);
        image.add_css_class("stat-icon");
        image.set_accessible_role(gtk::AccessibleRole::Presentation);
        card.append(&image);
        card.append(&widgets::label(&value, "stat-value"));
        card.append(&widgets::label(label, "stat-label"));
        card.update_property(&[gtk::accessible::Property::Label(&format!(
            "{label}: {value}"
        ))]);
        flow.insert(&card, -1);
    }
    let mut child = flow.first_child();
    while let Some(c) = child {
        c.set_focusable(false);
        child = c.next_sibling();
    }
}

fn fill_list(id: &str, list: &gtk::Box, home: &Home) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
    match id {
        "energy" => {
            for sensor in home
                .entities
                .values()
                .filter(|e| home_features::is_energy(e) && !e.hidden)
            {
                let value = widgets::label(&present::state_text(sensor), "num");
                let row = widgets::row(
                    &sensor.name(),
                    &glib_escape(sensor.area_name.as_deref().unwrap_or("")),
                    Some(value.upcast_ref()),
                );
                list.append(&row);
            }
        }
        _ => {
            let issues = present::health(home.entities.values());
            let total = issues.len();
            for issue in issues.into_iter().take(10) {
                let tag = widgets::tag(&issue.problem, "warning");
                list.append(&widgets::row(
                    &issue.title,
                    &glib_escape(&issue.place),
                    Some(tag.upcast_ref()),
                ));
            }
            if total > 10 {
                list.append(&widgets::row(
                    &format!("{} more", total - 10),
                    "Search for them in Rooms.",
                    None,
                ));
            }
        }
    }
}

fn glib_escape(text: &str) -> String {
    gtk::glib::markup_escape_text(text).to_string()
}

/// What to show instead of devices while Home Assistant isn't loaded.
pub fn set_empty(container: &gtk::Box, link: &Link) {
    clear(container);
    let action = |label: &'static str, run: fn()| -> nyx_ui::widgets::Action<'static> {
        Some((label, Box::new(run)))
    };
    let escaped = |text: &str| glib_escape(text);
    let state = match link {
        Link::NotSetUp => widgets::empty_state(
            "network-server-symbolic",
            "Connect Home Assistant",
            "Home shows your rooms, devices and automations from your Home Assistant server.",
            action("Connect…", connection::dialog),
        ),
        Link::Unlocking => widgets::empty_state(
            "dialog-password-symbolic",
            "Unlocking your keyring…",
            "Home keeps its Home Assistant token in your desktop keyring. Unlock it if you're asked.",
            None,
        ),
        Link::Connecting | Link::Online | Link::Offline(_) => {
            widgets::empty_state("network-wireless-symbolic", "Loading your home…", "", None)
        }
        Link::NeedsToken => widgets::empty_state(
            "dialog-password-symbolic",
            "Add an access token",
            "Home Assistant is set up, but no token is saved for it.",
            action("Add token…", connection::dialog),
        ),
        Link::Rejected => widgets::empty_state(
            "dialog-password-symbolic",
            "Home Assistant rejected the token",
            "The token may have been deleted or expired. Create a new one in your Home Assistant profile.",
            action("Replace token…", connection::dialog),
        ),
        Link::Locked(message) => widgets::empty_state(
            "dialog-password-symbolic",
            "Unlock your keyring",
            &escaped(message),
            action("Try again", connection::restore),
        ),
        Link::Unreachable(message) => widgets::empty_state(
            "network-offline-symbolic",
            "Can't reach Home Assistant",
            &format!("{} Home keeps trying.", escaped(message)),
            action("Check connection…", connection::dialog),
        ),
    };
    container.append(&state);
}
