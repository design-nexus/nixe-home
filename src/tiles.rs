//! Device tiles: one card per entity, with the control that fits it. A tile is
//! built once and then updated in place as its state changes, so focus and
//! scroll position survive live updates.
use crate::{
    app,
    ha::{self, Entity},
    home_features::{self, RobotAction},
    prefs,
    present::{self, Primary},
    rt,
};
use gtk::{gdk, glib, prelude::*};
use nyx_ui::widgets;
use serde_json::json;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

pub struct Tile {
    pub child: gtk::FlowBoxChild,
    pub signature: String,
    entity: Rc<RefCell<Entity>>,
    card: gtk::Box,
    icon: gtk::Image,
    name: gtk::Label,
    state: gtk::Label,
    toggle: Option<(gtk::Switch, glib::SignalHandlerId)>,
    lock: Option<gtk::Button>,
    brightness: Option<(gtk::Scale, Rc<Cell<bool>>)>,
    target: Option<gtk::Label>,
    media: Option<gtk::Button>,
    covers: Vec<gtk::Button>,
    more: gtk::MenuButton,
}

impl Tile {
    pub fn new(entity: &Entity) -> Self {
        let shared = Rc::new(RefCell::new(entity.clone()));
        let id = entity.entity_id.clone();
        let card = widgets::vbox(10);
        card.add_css_class("tile");
        let top = widgets::hbox(12);
        let icon = gtk::Image::from_icon_name(present::icon(entity));
        icon.add_css_class("tile-icon");
        icon.set_valign(gtk::Align::Center);
        icon.set_accessible_role(gtk::AccessibleRole::Presentation);
        top.append(&icon);
        let text = widgets::vbox(2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        // Bounded natural widths keep every tile the same width, so grids of
        // tall and one-line tiles line up in the same columns.
        let name = widgets::label(&entity.name(), "tile-name");
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        name.set_max_width_chars(14);
        let state = widgets::label("", "tile-state");
        state.set_ellipsize(gtk::pango::EllipsizeMode::End);
        state.set_max_width_chars(14);
        text.append(&name);
        text.append(&state);
        top.append(&text);

        let mut toggle = None;
        let mut lock = None;
        match present::primary(entity) {
            Primary::Toggle(on) => {
                let switch = gtk::Switch::new();
                switch.set_active(on);
                switch.set_valign(gtk::Align::Center);
                switch.update_property(&[gtk::accessible::Property::Label(&entity.name())]);
                let id = id.clone();
                let handler = switch.connect_state_set(move |_, on| {
                    app::call(&id, present::toggle_service(on), json!({}));
                    glib::Propagation::Proceed
                });
                top.append(&switch);
                toggle = Some((switch, handler));
            }
            Primary::Lock { .. } => {
                let button = gtk::Button::new();
                button.set_valign(gtk::Align::Center);
                let armed = Rc::new(Cell::new(false));
                let shared = shared.clone();
                button.connect_clicked(move |button| {
                    let entity = shared.borrow().clone();
                    let locked = entity.state == "locked";
                    if !locked {
                        app::call(&entity.entity_id, "lock", json!({}));
                        return;
                    }
                    if !armed.replace(true) {
                        button.set_label("Unlock?");
                        button.add_css_class("destructive-action");
                        let (button, armed) = (button.clone(), armed.clone());
                        glib::timeout_add_local_once(Duration::from_secs(4), move || {
                            if armed.replace(false) {
                                button.set_label("Unlock");
                                button.remove_css_class("destructive-action");
                            }
                        });
                        return;
                    }
                    armed.set(false);
                    button.set_label("Unlock");
                    button.remove_css_class("destructive-action");
                    app::call(&entity.entity_id, "unlock", json!({}));
                });
                top.append(&button);
                lock = Some(button);
            }
            Primary::Press { label, service } => {
                let button = gtk::Button::with_label(label);
                button.set_valign(gtk::Align::Center);
                button.update_property(&[gtk::accessible::Property::Label(&format!(
                    "{label} {}",
                    entity.name()
                ))]);
                let id = id.clone();
                button.connect_clicked(move |_| app::call(&id, service, json!({})));
                top.append(&button);
            }
            Primary::None => {}
        }

        let more = gtk::MenuButton::new();
        more.set_icon_name("view-more-symbolic");
        more.add_css_class("bar-button");
        more.add_css_class("tile-more");
        more.set_valign(gtk::Align::Center);
        more.set_tooltip_text(Some("Details and options (Shift+F10)"));
        more.update_property(&[gtk::accessible::Property::Label(&format!(
            "Details for {}",
            entity.name()
        ))]);
        {
            let shared = shared.clone();
            more.set_create_popup_func(move |button| {
                button.set_popover(Some(&details_popover(&shared.borrow())));
            });
        }
        top.append(&more);
        card.append(&top);

        let brightness = present::brightness(entity).map(|_| {
            let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0);
            scale.set_draw_value(false);
            scale.set_hexpand(true);
            scale.set_size_request(60, -1);
            scale.update_property(&[gtk::accessible::Property::Label(&format!(
                "Brightness of {}",
                entity.name()
            ))]);
            let syncing = Rc::new(Cell::new(false));
            let pending = Rc::new(Cell::new(0u32));
            let id = id.clone();
            let guard = syncing.clone();
            scale.connect_value_changed(move |scale| {
                if guard.get() {
                    return;
                }
                let ticket = pending.get().wrapping_add(1);
                pending.set(ticket);
                let (pending, id, value) = (pending.clone(), id.clone(), scale.value().round());
                glib::timeout_add_local_once(Duration::from_millis(300), move || {
                    if pending.get() == ticket {
                        app::call(&id, "turn_on", json!({"brightness_pct": value as i64}));
                    }
                });
            });
            card.append(&scale);
            (scale, syncing)
        });

        let target = present::climate(entity).map(|_| {
            let row = widgets::hbox(8);
            let lower =
                widgets::icon_button("list-remove-symbolic", "Lower the target temperature");
            let value = widgets::label("", "tile-readout");
            value.set_hexpand(true);
            value.set_xalign(0.5);
            let raise = widgets::icon_button("list-add-symbolic", "Raise the target temperature");
            for (button, up) in [(&lower, false), (&raise, true)] {
                let (shared, value) = (shared.clone(), value.clone());
                button.connect_clicked(move |_| {
                    let entity = shared.borrow().clone();
                    let Some(climate) = present::climate(&entity) else {
                        return;
                    };
                    let next =
                        present::climate_target(&climate, up).clamp(climate.min, climate.max);
                    value.set_text(&format!("Target {next}°"));
                    shared.borrow_mut().attributes["temperature"] = json!(next);
                    app::call(
                        &entity.entity_id,
                        "set_temperature",
                        json!({"temperature": next}),
                    );
                });
            }
            row.append(&lower);
            row.append(&value);
            row.append(&raise);
            card.append(&row);
            value
        });

        let mut covers = Vec::new();
        let cover_actions = present::cover_actions(entity);
        if !cover_actions.is_empty() {
            let row = widgets::hbox(6);
            row.add_css_class("tile-actions");
            for (label, service, icon) in cover_actions {
                let button = widgets::icon_button(icon, label);
                button.update_property(&[gtk::accessible::Property::Label(&format!(
                    "{label} {}",
                    entity.name()
                ))]);
                let id = id.clone();
                button.connect_clicked(move |_| app::call(&id, service, json!({})));
                row.append(&button);
                covers.push(button);
            }
            card.append(&row);
        }

        let media = present::media(entity).map(|_| {
            let button = widgets::icon_button("media-playback-start-symbolic", "Play or pause");
            button.set_halign(gtk::Align::Start);
            let id = id.clone();
            button.connect_clicked(move |_| app::call(&id, "media_play_pause", json!({})));
            card.append(&button);
            button
        });

        if matches!(entity.domain(), "vacuum" | "lawn_mower") {
            card.append(&robot_controls(&shared));
        }
        if entity.domain() == "camera" {
            card.append(&picture(&shared, "Show preview"));
        }

        let child = gtk::FlowBoxChild::new();
        child.set_child(Some(&card));
        child.set_widget_name(&id);
        install_context(&child, &more);

        let tile = Self {
            child,
            signature: present::signature(entity),
            entity: shared,
            card,
            icon,
            name,
            state,
            toggle,
            lock,
            brightness,
            target,
            media,
            covers,
            more,
        };
        tile.update(entity);
        tile
    }

    pub fn update(&self, entity: &Entity) {
        *self.entity.borrow_mut() = entity.clone();
        let name = entity.name();
        let state = present::state_text(entity);
        self.name.set_text(&present::short_name(entity));
        self.state.set_text(&present::tile_detail(entity));
        self.icon.set_icon_name(Some(present::icon(entity)));
        self.child
            .update_property(&[gtk::accessible::Property::Label(&format!(
                "{name}, {state}"
            ))]);
        self.child
            .set_tooltip_text(Some(&format!("{name} · {state}")));
        let on = matches!(
            entity.state.as_str(),
            "on" | "open"
                | "playing"
                | "heat"
                | "cool"
                | "heat_cool"
                | "cleaning"
                | "mowing"
                | "unlocked"
        );
        set_class(&self.card, "on", on);
        set_class(&self.card, "unavailable", present::unavailable(entity));
        if let Some((switch, handler)) = &self.toggle {
            switch.block_signal(handler);
            switch.set_active(entity.state == "on");
            switch.set_state(entity.state == "on");
            switch.unblock_signal(handler);
        }
        if let Some(button) = &self.lock {
            if !button.has_css_class("destructive-action") {
                button.set_label(if entity.state == "locked" {
                    "Unlock"
                } else {
                    "Lock"
                });
            }
            button.update_property(&[gtk::accessible::Property::Label(&format!(
                "{} {name}",
                if entity.state == "locked" {
                    "Unlock"
                } else {
                    "Lock"
                }
            ))]);
        }
        if let Some((scale, syncing)) = &self.brightness {
            syncing.set(true);
            let pct = present::brightness(entity).unwrap_or(0.0);
            scale.set_value(pct.max(1.0));
            scale.set_sensitive(!present::unavailable(entity));
            syncing.set(false);
        }
        if let Some(label) = &self.target {
            label.set_text(
                &present::climate(entity)
                    .map(|c| format!("Target {}°", c.target))
                    .unwrap_or_else(|| "No target".into()),
            );
        }
        if let Some(button) = &self.media {
            button.set_icon_name(if entity.state == "playing" {
                "media-playback-pause-symbolic"
            } else {
                "media-playback-start-symbolic"
            });
        }
        for button in &self.covers {
            button.set_sensitive(!present::unavailable(entity));
        }
    }

    /// The tile's main action, for Enter and Space.
    pub fn activate(&self) {
        if let Some((switch, _)) = &self.toggle {
            switch.set_active(!switch.is_active());
        } else {
            self.more.popup();
        }
    }
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

/// Right-click, the Menu key and Shift+F10 open a tile's details.
fn install_context(child: &gtk::FlowBoxChild, more: &gtk::MenuButton) {
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    let m = more.clone();
    click.connect_pressed(move |_, _, _, _| m.popup());
    child.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    let m = more.clone();
    keys.connect_key_pressed(move |_, key, _, mods| {
        if key == gdk::Key::Menu
            || (key == gdk::Key::F10 && mods.contains(gdk::ModifierType::SHIFT_MASK))
        {
            m.popup();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    child.add_controller(keys);
}

fn details_popover(entity: &Entity) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let body = widgets::vbox(8);
    body.set_margin_top(8);
    body.set_margin_bottom(8);
    body.set_margin_start(10);
    body.set_margin_end(10);
    body.set_width_request(260);
    body.append(&widgets::label(&entity.name(), "card-title"));
    let grid = gtk::Grid::new();
    grid.set_column_spacing(12);
    grid.set_row_spacing(4);
    for (row, (key, value)) in present::details(entity).into_iter().enumerate() {
        let k = widgets::label(&key, "dim");
        let v = widgets::label(&value, if key == "Entity" { "mono" } else { "" });
        v.set_selectable(true);
        v.set_wrap(true);
        v.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        v.set_hexpand(true);
        grid.attach(&k, 0, row as i32, 1, 1);
        grid.attach(&v, 1, row as i32, 1, 1);
    }
    body.append(&grid);
    let favorite = prefs::get().favorite_entities.contains(&entity.entity_id);
    let star = widgets::labeled_button(
        if favorite {
            "starred-symbolic"
        } else {
            "non-starred-symbolic"
        },
        if favorite {
            "Remove from Overview"
        } else {
            "Add to Overview"
        },
    );
    star.add_css_class("flat");
    let id = entity.entity_id.clone();
    let p = popover.clone();
    star.connect_clicked(move |_| {
        p.popdown();
        toggle_favorite(&id);
    });
    body.append(&star);
    popover.set_child(Some(&body));
    popover
}

pub fn toggle_favorite(entity_id: &str) {
    let mut added = false;
    prefs::update(|p| {
        if p.favorite_entities.iter().any(|id| id == entity_id) {
            p.favorite_entities.retain(|id| id != entity_id);
        } else {
            p.favorite_entities.push(entity_id.to_string());
            added = true;
        }
    });
    app::rebuild();
    app::toast(if added {
        "Added to Overview."
    } else {
        "Removed from Overview."
    });
}

/// Start, pause, dock and the other robot commands the entity supports.
fn robot_controls(shared: &Rc<RefCell<Entity>>) -> gtk::Box {
    let entity = shared.borrow().clone();
    let column = widgets::vbox(8);
    let row = gtk::FlowBox::new();
    row.set_selection_mode(gtk::SelectionMode::None);
    row.set_column_spacing(6);
    row.set_row_spacing(6);
    row.set_max_children_per_line(4);
    for action in home_features::robot_actions(&entity) {
        let button = gtk::Button::with_label(action.label(entity.domain()));
        let shared = shared.clone();
        button.connect_clicked(move |_| {
            let entity = shared.borrow().clone();
            if action == RobotAction::CleanArea {
                choose_area(&entity);
            } else if entity.domain() == "lawn_mower" && action == RobotAction::Start {
                confirm_mowing(&entity);
            } else {
                robot(&entity, action, None);
            }
        });
        row.insert(&button, -1);
    }
    let mut child = row.first_child();
    while let Some(c) = child {
        c.set_focusable(false);
        child = c.next_sibling();
    }
    column.append(&row);
    if entity.domain() == "vacuum"
        && let Some(map) = app::get().and_then(|app| {
            app.home
                .borrow()
                .entities
                .values()
                .find(|c| {
                    c.domain() == "image"
                        && c.device_id.is_some()
                        && c.device_id == entity.device_id
                        && c.name().to_lowercase().contains("map")
                })
                .cloned()
        })
    {
        column.append(&picture(&Rc::new(RefCell::new(map)), "Show map"));
    }
    column
}

fn robot(entity: &Entity, action: RobotAction, area: Option<String>) {
    let Some(app) = app::get() else { return };
    let (base, token) = {
        let home = app.home.borrow();
        (home.base.clone(), home.token.clone())
    };
    let entity = entity.clone();
    rt::spawn(
        async move { ha::call_robot_service(&base, &token, &entity, action, area.as_deref()).await },
        |result| match result {
            Ok(()) => app::toast("Command sent."),
            Err(error) => app::toast(&error),
        },
    );
}

fn choose_area(entity: &Entity) {
    let Some(app) = app::get() else { return };
    let areas = app.home.borrow().areas.clone();
    if areas.is_empty() {
        app::toast("Home Assistant has no areas to clean.");
        return;
    }
    let (dialog, card) = widgets::dialog("Clean an area", 380);
    let note = widgets::label(
        &format!(
            "{} cleans the area you choose. Map areas to the vacuum in Home Assistant first.",
            entity.name()
        ),
        "dim",
    );
    note.set_wrap(true);
    card.append(&note);
    let names: Vec<&str> = areas.iter().map(|a| a.name.as_str()).collect();
    let picker = gtk::DropDown::from_strings(&names);
    picker.update_property(&[gtk::accessible::Property::Label("Area to clean")]);
    card.append(&picker);
    let buttons = widgets::hbox(8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let clean = gtk::Button::with_label("Clean");
    clean.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&clean);
    card.append(&buttons);
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let (d, entity) = (dialog.clone(), entity.clone());
    clean.connect_clicked(move |_| {
        if let Some(area) = areas.get(picker.selected() as usize) {
            robot(&entity, RobotAction::CleanArea, Some(area.id.clone()));
        }
        d.close();
    });
    dialog.present();
}

fn confirm_mowing(entity: &Entity) {
    let (dialog, card) = widgets::dialog(&format!("Start {}?", entity.name()), 380);
    let note = widgets::label(
        "Check that the lawn is clear of people, pets and objects, and that the mower's safety features are on.",
        "dim",
    );
    note.set_wrap(true);
    card.append(&note);
    let buttons = widgets::hbox(8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let start = gtk::Button::with_label("Start mowing");
    start.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&start);
    card.append(&buttons);
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let (d, entity) = (dialog.clone(), entity.clone());
    start.connect_clicked(move |_| {
        robot(&entity, RobotAction::Start, None);
        d.close();
    });
    dialog.present();
    cancel.grab_focus();
}

/// A still image from a camera or image entity, loaded only when asked for.
fn picture(shared: &Rc<RefCell<Entity>>, action: &str) -> gtk::Box {
    let entity = shared.borrow().clone();
    let column = widgets::vbox(6);
    let picture = gtk::Picture::new();
    picture.add_css_class("tile-picture");
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_size_request(-1, 150);
    picture.set_alternative_text(Some(&format!("Still image from {}", entity.name())));
    let cached =
        app::get().and_then(|app| app.home.borrow().previews.get(&entity.entity_id).cloned());
    picture.set_visible(cached.is_some());
    if let Some(texture) = cached {
        picture.set_paintable(Some(&texture));
    }
    let button = widgets::labeled_button("camera-photo-symbolic", action);
    button.set_halign(gtk::Align::Start);
    button.set_sensitive(!matches!(
        entity.state.as_str(),
        "unavailable" | "unknown" | "off"
    ));
    let (shared, image) = (shared.clone(), picture.clone());
    button.connect_clicked(move |button| {
        let Some(app) = app::get() else { return };
        let (base, token) = {
            let home = app.home.borrow();
            (home.base.clone(), home.token.clone())
        };
        let entity = shared.borrow().clone();
        button.set_sensitive(false);
        let (button, image) = (button.clone(), image.clone());
        rt::spawn(
            async move {
                let bytes = ha::fetch_entity_picture(&base, &token, &entity).await;
                (entity.entity_id, bytes)
            },
            move |(id, result)| {
                button.set_sensitive(true);
                match result.and_then(|bytes| {
                    gdk::Texture::from_bytes(&glib::Bytes::from(&bytes))
                        .map_err(|_| "Couldn't read the image.".to_string())
                }) {
                    Ok(texture) => {
                        image.set_paintable(Some(&texture));
                        image.set_visible(true);
                        if let Some(app) = app::get() {
                            app.home.borrow_mut().previews.insert(id, texture);
                        }
                    }
                    Err(error) => app::toast(&error),
                }
            },
        );
    });
    column.append(&picture);
    column.append(&button);
    column
}

/// Whether a tile has controls under its name (a slider, buttons, a picture),
/// which makes it taller than a one-line tile.
pub fn is_rich(entity: &Entity) -> bool {
    present::brightness(entity).is_some()
        || present::climate(entity).is_some()
        || !present::cover_actions(entity).is_empty()
        || present::media(entity).is_some()
        || matches!(entity.domain(), "vacuum" | "lawn_mower" | "camera")
}

/// A flowing grid for tiles: arrow keys move between them, Enter and Space
/// use the focused tile's main control. Tiles in a grid share one size, so
/// tall and one-line tiles go in separate grids.
pub fn grid() -> gtk::FlowBox {
    let flow = gtk::FlowBox::new();
    flow.add_css_class("tile-grid");
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_homogeneous(true);
    flow.set_min_children_per_line(1);
    flow.set_max_children_per_line(6);
    flow.set_column_spacing(12);
    flow.set_row_spacing(12);
    flow.set_valign(gtk::Align::Start);
    flow.set_activate_on_single_click(false);
    flow
}
