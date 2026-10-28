//! Device cards: one card per device with what's useful at a glance, and the
//! rest in its details. Cards are built once; live changes reach them through
//! watchers, so focus and scroll position survive updates.
use crate::{
    app::{self, Home},
    details,
    device::{self, Device, Kind},
    grid::{self, Size},
    ha::{self, Entity},
    home_features::{self, RobotAction},
    icons, present, robot_window, rt, viewer,
};
use gtk::{gdk, glib, prelude::*};
use nyx_ui::widgets;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap},
    rc::Rc,
    time::Duration,
};

type Update = Rc<dyn Fn(&Home)>;
/// Marks a choice without calling back.
type Setter = Rc<dyn Fn(&str)>;

struct Watcher {
    widget: glib::WeakRef<gtk::Widget>,
    ids: Vec<String>,
    update: Update,
}

thread_local! {
    static WATCHERS: RefCell<Vec<Watcher>> = const { RefCell::new(Vec::new()) };
    static ACTIVATE: RefCell<HashMap<String, Rc<dyn Fn()>>> = RefCell::new(HashMap::new());
}

/// Run `update` whenever one of `ids` changes, for as long as `widget` lives.
pub fn watch(widget: &impl IsA<gtk::Widget>, ids: Vec<String>, update: impl Fn(&Home) + 'static) {
    WATCHERS.with(|w| {
        w.borrow_mut().push(Watcher {
            widget: widget.upcast_ref::<gtk::Widget>().downgrade(),
            ids,
            update: Rc::new(update),
        })
    });
}

/// Bring every live card and details window up to date with `changed`
/// entities (all of them when `None`).
pub fn notify(home: &Home, changed: Option<&BTreeSet<String>>) {
    let due: Vec<Update> = WATCHERS.with(|w| {
        let mut w = w.borrow_mut();
        // A card out of the window is gone for good; its watcher holds it alive.
        w.retain(|x| x.widget.upgrade().is_some_and(|w| w.root().is_some()));
        w.iter()
            .filter(|x| changed.is_none_or(|c| x.ids.iter().any(|id| c.contains(id))))
            .map(|x| x.update.clone())
            .collect()
    });
    for update in due {
        update(home);
    }
}

/// Enter or Space on a card: its main toggle, or its details.
pub fn activate(device_id: &str) {
    if let Some(f) = ACTIVATE.with(|a| a.borrow().get(device_id).cloned()) {
        f();
    }
}

fn set_class(widget: &impl IsA<gtk::Widget>, class: &str, on: bool) {
    if on {
        widget.add_css_class(class);
    } else {
        widget.remove_css_class(class);
    }
}

fn entity<'a>(home: &'a Home, id: &str) -> Option<&'a Entity> {
    home.entities.get(id)
}

fn features(e: &Entity) -> u64 {
    e.attributes["supported_features"].as_u64().unwrap_or(0)
}

/// Calls `f` once input settles, so a dragged slider sends one request.
#[derive(Clone, Default)]
pub struct Debounce(Rc<Cell<u32>>);

impl Debounce {
    pub fn run(&self, ms: u64, f: impl FnOnce() + 'static) {
        let ticket = self.0.get().wrapping_add(1);
        self.0.set(ticket);
        let me = self.0.clone();
        glib::timeout_add_local_once(Duration::from_millis(ms), move || {
            if me.get() == ticket {
                f();
            }
        });
    }
}

/// Where a light command goes: one entity (a light or a light group), or
/// every light in an area.
#[derive(Clone)]
enum Target {
    Entity(String),
    Area(String),
}

impl Target {
    fn call(&self, service: &str, data: Value) {
        match self {
            Self::Entity(id) => app::call(id, service, data),
            Self::Area(area) => app::call_area(area, service, data),
        }
    }
}

/// A switch for an entity's on/off, quiet while it's being set from state.
pub fn switch(entity_id: &str, label: &str) -> (gtk::Switch, Rc<dyn Fn(bool)>) {
    target_switch(Target::Entity(entity_id.to_string()), label)
}

fn target_switch(target: Target, label: &str) -> (gtk::Switch, Rc<dyn Fn(bool)>) {
    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Center);
    switch.update_property(&[gtk::accessible::Property::Label(label)]);
    let handler = switch.connect_state_set(move |_, on| {
        target.call(present::toggle_service(on), json!({}));
        glib::Propagation::Proceed
    });
    let s = switch.clone();
    let set = Rc::new(move |on: bool| {
        s.block_signal(&handler);
        s.set_active(on);
        s.set_state(on);
        s.unblock_signal(&handler);
    });
    (switch, set)
}

/// Joined buttons where exactly one is chosen; `set` marks one without
/// calling back.
fn choice(
    options: &[(String, String)],
    label: &str,
    pick: impl Fn(&str) + 'static,
) -> (gtk::Box, Setter) {
    let row = widgets::hbox(0);
    row.add_css_class("segmented");
    row.add_css_class("card-choice");
    row.update_property(&[gtk::accessible::Property::Label(label)]);
    let quiet = Rc::new(Cell::new(false));
    let pick = Rc::new(pick);
    let mut buttons: Vec<(String, gtk::ToggleButton)> = Vec::new();
    for (id, name) in options {
        let b = gtk::ToggleButton::with_label(name);
        b.add_css_class("segment");
        b.set_hexpand(true);
        if let Some((_, first)) = buttons.first() {
            b.set_group(Some(first));
        }
        let (id2, quiet2, pick2) = (id.clone(), quiet.clone(), pick.clone());
        b.connect_toggled(move |b| {
            if b.is_active() && !quiet2.get() {
                pick2(&id2);
            }
        });
        row.append(&b);
        buttons.push((id.clone(), b));
    }
    let set = Rc::new(move |current: &str| {
        quiet.set(true);
        for (id, b) in &buttons {
            if id == current {
                b.set_active(true);
            }
        }
        if !buttons.iter().any(|(id, _)| id == current) {
            for (_, b) in &buttons {
                b.set_active(false);
            }
        }
        quiet.set(false);
    });
    (row, set)
}

fn sentence(text: &str) -> String {
    present::sentence_case(text)
}

/// "5 min ago" from an ISO timestamp state or attribute.
pub fn ago(iso: &str) -> Option<String> {
    let then = glib::DateTime::from_iso8601(iso, None).ok()?.to_unix();
    let now = glib::DateTime::now_utc().ok()?.to_unix();
    Some(present::relative_time(then, now))
}

struct Frame {
    card: gtk::Box,
    more: gtk::Button,
    head_end: gtk::Box,
    sub: gtk::Label,
}

/// The parts every card shares: a colored icon chip, the name, a status line,
/// and a button for the details.
fn frame(device: &Device) -> Frame {
    let card = widgets::vbox(10);
    card.add_css_class("device-card");
    card.add_css_class(device.kind.css());
    let head = widgets::hbox(12);
    let chip = gtk::Image::from_icon_name(device.kind.icon());
    chip.add_css_class("chip");
    chip.set_valign(gtk::Align::Center);
    chip.set_accessible_role(gtk::AccessibleRole::Presentation);
    head.append(&chip);
    let titles = widgets::vbox(2);
    titles.set_hexpand(true);
    titles.set_valign(gtk::Align::Center);
    let name = widgets::label(&device.name, "card-name");
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(16);
    let sub = widgets::label("", "card-sub");
    sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
    sub.set_max_width_chars(16);
    titles.append(&name);
    titles.append(&sub);
    head.append(&titles);
    let head_end = widgets::hbox(6);
    head_end.set_valign(gtk::Align::Center);
    head.append(&head_end);
    let more = widgets::bar_button("view-more-symbolic", "Details and settings (Shift+F10)");
    more.update_property(&[gtk::accessible::Property::Label(&format!(
        "Details for {}",
        device.name
    ))]);
    let id = device.id.clone();
    more.connect_clicked(move |_| details::open(&id));
    head.append(&more);
    if device.hidden || device.grouped {
        card.add_css_class("hidden-device");
        let tag = widgets::label(
            if device.hidden {
                "Hidden"
            } else {
                "In a group"
            },
            "hidden-tag",
        );
        tag.set_valign(gtk::Align::Center);
        head_end.append(&tag);
    }
    card.append(&head);
    card.set_focusable(true);
    card.set_widget_name(&device.id);
    // Right-click, the Menu key and Shift+F10 open the details; Enter and
    // Space on the card itself use its main control.
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    let id = device.id.clone();
    click.connect_pressed(move |_, _, _, _| details::open(&id));
    card.add_controller(click);
    let keys = gtk::EventControllerKey::new();
    let (id, me) = (device.id.clone(), card.downgrade());
    keys.connect_key_pressed(move |_, key, _, mods| {
        if key == gdk::Key::Menu
            || (key == gdk::Key::F10 && mods.contains(gdk::ModifierType::SHIFT_MASK))
        {
            details::open(&id);
            return glib::Propagation::Stop;
        }
        let on_card = me.upgrade().is_some_and(|c| c.has_focus());
        if on_card && matches!(key, gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space) {
            activate(&id);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    card.add_controller(keys);
    Frame {
        card,
        more,
        head_end,
        sub,
    }
}

/// Keep the status line, accessible name and on/unavailable look current.
fn status(
    frame: &Frame,
    device: &Device,
    show_area: bool,
    text: impl Fn(&Home) -> (String, bool, bool) + 'static,
) {
    let (sub, card, name, area) = (
        frame.sub.clone(),
        frame.card.clone(),
        device.name.clone(),
        device.area.clone().filter(|_| show_area),
    );
    watch(&frame.card, device.entities.clone(), move |home| {
        let (line, on, unavailable) = text(home);
        let full = match &area {
            Some(area) if !line.is_empty() => format!("{area} · {line}"),
            Some(area) => area.clone(),
            None => line.clone(),
        };
        sub.set_text(&full);
        set_class(&card, "on", on);
        set_class(&card, "unavailable", unavailable);
        card.update_property(&[gtk::accessible::Property::Label(&format!("{name}, {full}"))]);
        card.set_tooltip_text(Some(&format!("{name} · {full}")));
    });
}

const S14: Size = Size::new(1, 4);
const S24: Size = Size::new(2, 4);
const S44: Size = Size::new(4, 4);
const S22: Size = Size::new(2, 2);
const S12: Size = Size::new(1, 2);

/// The sizes a device's card comes in, its default first.
pub fn size_options(device: &Device, home: &Home) -> Vec<Size> {
    match device.kind {
        Kind::Light | Kind::Media | Kind::Fan => vec![S24, S14],
        Kind::Climate | Kind::Camera => vec![S44, S24],
        Kind::Robot => vec![S24, S44],
        Kind::Security => vec![S24],
        Kind::Lock | Kind::Cover => vec![S14],
        Kind::Switch => {
            if toggles(device, home).len() > 1 || !buttons(device, home).is_empty() {
                vec![S24, S14]
            } else {
                vec![S14, S24]
            }
        }
        Kind::Sensors => {
            if device.readings(&home.entities, 2).len() > 1 {
                vec![S24, S14]
            } else {
                vec![S14, S24]
            }
        }
    }
}

/// The card for `device` at `size`, filled in from `home`.
pub fn card(device: &Device, home: &Home, show_area: bool, size: Size) -> gtk::Widget {
    let frame = frame(device);
    frame.card.add_css_class(&format!("size-h{}", size.h));
    match device.kind {
        Kind::Camera => camera(&frame, device, home, show_area, size),
        Kind::Climate => climate(&frame, device, home, show_area, size),
        Kind::Light => light(&frame, device, home, show_area, size),
        Kind::Robot => robot_card(&frame, device, home, show_area, size),
        Kind::Media => media(&frame, device, home, show_area, size),
        Kind::Fan => fan(&frame, device, home, show_area, size),
        Kind::Security => security(&frame, device, home, show_area),
        Kind::Lock => lock(&frame, device, home, show_area),
        Kind::Cover => cover(&frame, device, home, show_area),
        Kind::Switch => switches(&frame, device, home, show_area, size),
        Kind::Sensors => sensors(&frame, device, home, show_area, size),
    }
    finish(&frame, size);
    // Fill everything in from the current state.
    notify_one(frame.card.upcast_ref(), home);
    frame.card.upcast()
}

/// In a card two or more rows tall, the last part sits at the bottom.
fn finish(frame: &Frame, size: Size) {
    if size.h >= 2
        && let Some(last) = frame.card.last_child()
        && frame.card.first_child().as_ref() != Some(&last)
    {
        last.set_vexpand(true);
        last.set_valign(gtk::Align::End);
    }
}

/// Run the watchers of one new card.
fn notify_one(widget: &gtk::Widget, home: &Home) {
    let mine: Vec<Update> = WATCHERS.with(|w| {
        w.borrow()
            .iter()
            .filter(|x| x.widget.upgrade().is_some_and(|wd| wd == *widget))
            .map(|x| x.update.clone())
            .collect()
    });
    for update in mine {
        update(home);
    }
}

fn set_activate(device: &Device, f: impl Fn() + 'static) {
    ACTIVATE.with(|a| a.borrow_mut().insert(device.id.clone(), Rc::new(f)));
}

fn open_details_on_activate(device: &Device) {
    let id = device.id.clone();
    set_activate(device, move || details::open(&id));
}

// ---------- Camera ----------

fn camera(frame: &Frame, device: &Device, home: &Home, show_area: bool, size: Size) {
    let Some(cam) = device.primary(&home.entities).cloned() else {
        return;
    };
    frame.card.add_css_class("camera-card");
    if size.h == 4 {
        frame
            .card
            .prepend(&viewer::thumbnail(&cam.entity_id, &device.name, 176));
    }
    let enabled = device
        .find(&home.entities, |e| {
            e.domain() == "switch"
                && e.entity_category.is_none()
                && e.name().to_lowercase().ends_with("enabled")
        })
        .map(|e| e.entity_id.clone());
    if let Some(id) = &enabled {
        let (sw, set) = switch(id, &format!("{} enabled", device.name));
        sw.set_tooltip_text(Some("Camera on or off"));
        frame.head_end.append(&sw);
        let id = id.clone();
        watch(&frame.card, vec![id.clone()], move |home| {
            if let Some(e) = entity(home, &id) {
                set(e.state == "on");
            }
        });
    }
    let chips = widgets::hbox(6);
    chips.add_css_class("indicators");
    frame.card.append(&chips);
    let find = |pred: &dyn Fn(&Entity) -> bool| {
        device
            .find(&home.entities, pred)
            .map(|e| e.entity_id.clone())
    };
    let motion = find(&|e| e.domain() == "binary_sensor" && device::device_class(e) == "motion");
    let person = find(&|e| e.domain() == "binary_sensor" && device::device_class(e) == "occupancy");
    let streaming =
        find(&|e| e.domain() == "binary_sensor" && e.name().to_lowercase().ends_with("streaming"));
    let charging =
        find(&|e| e.domain() == "binary_sensor" && device::device_class(e) == "battery_charging");
    let battery = find(&|e| e.domain() == "sensor" && device::device_class(e) == "battery");
    let detection =
        find(&|e| e.domain() == "event" && e.name().to_lowercase().ends_with("detection"));
    let ring = find(&|e| e.domain() == "event" && device::device_class(e) == "doorbell");
    let make = |icon: &str, text: &str| {
        let chip = widgets::hbox(4);
        chip.add_css_class("indicator");
        chip.append(&gtk::Image::from_icon_name(icon));
        let l = widgets::label(text, "");
        chip.append(&l);
        chips.append(&chip);
        (chip, l)
    };
    let motion_chip = make("view-reveal-symbolic", "Motion");
    let person_chip = make("avatar-default-symbolic", "Person");
    let stream_chip = make("media-record-symbolic", "Live");
    let plugged = device.plugged;
    let battery_chip = if plugged {
        make("ac-adapter-symbolic", "Plugged in")
    } else {
        make("battery-good-symbolic", "")
    };
    let cam_id = cam.entity_id.clone();
    status(frame, device, show_area, move |home| {
        let on = |id: &Option<String>| {
            id.as_ref()
                .and_then(|id| entity(home, id))
                .is_some_and(|e| e.state == "on")
        };
        motion_chip.0.set_visible(on(&motion));
        set_class(&motion_chip.0, "active", on(&motion));
        person_chip.0.set_visible(on(&person));
        set_class(&person_chip.0, "active", on(&person));
        stream_chip.0.set_visible(on(&streaming));
        let level = battery
            .as_ref()
            .and_then(|id| entity(home, id))
            .and_then(|e| e.state.parse::<f64>().ok());
        battery_chip.0.set_visible(plugged || level.is_some());
        if let Some(level) = level.filter(|_| !plugged) {
            battery_chip.1.set_text(&format!(
                "{}%{}",
                level.round(),
                if on(&charging) { " · charging" } else { "" }
            ));
            set_class(&battery_chip.0, "warning", level <= 20.0);
        }
        let when = |id: &Option<String>| {
            id.as_ref()
                .and_then(|id| entity(home, id))
                .and_then(|e| ago(&e.state))
        };
        let line = if on(&motion) || on(&person) {
            "Motion now".to_string()
        } else if let Some(t) = when(&ring) {
            format!("Rang {}", t.to_lowercase())
        } else if let Some(t) = when(&detection) {
            format!("Motion {}", t.to_lowercase())
        } else {
            entity(home, &cam_id)
                .map(present::state_text)
                .unwrap_or_default()
        };
        let unavailable = entity(home, &cam_id).is_none_or(present::unavailable);
        (line, on(&streaming), unavailable)
    });
    let (id, name) = (cam.entity_id.clone(), device.name.clone());
    set_activate(device, move || viewer::open(&id, &name, None));
}

// ---------- Climate ----------

fn temp(v: f64) -> String {
    if (v - v.round()).abs() < 0.05 {
        format!("{}°", v.round() as i64)
    } else {
        format!("{v:.1}°")
    }
}

fn mode_label(mode: &str) -> String {
    match mode {
        "heat_cool" | "auto" => "Auto".into(),
        "fan_only" => "Fan".into(),
        other => sentence(other),
    }
}

fn climate(frame: &Frame, device: &Device, home: &Home, show_area: bool, size: Size) {
    let Some(c) = device.primary(&home.entities).cloned() else {
        return;
    };
    // At 2x4: the readout and setpoints on one row, the mode in the status line.
    let compact = size.h < 4;
    let id = c.entity_id.clone();
    let readout = widgets::hbox(14);
    let current = widgets::label("–", "big-readout");
    readout.append(&current);
    let side = widgets::vbox(2);
    side.set_valign(gtk::Align::Center);
    let action = widgets::label("", "climate-action");
    let humidity = widgets::label("", "card-sub");
    side.append(&action);
    side.append(&humidity);
    if !compact {
        readout.append(&side);
    }
    frame.card.append(&readout);

    let modes: Vec<(String, String)> = c.attributes["hvac_modes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|m| (m.to_string(), mode_label(m)))
        .collect();
    let set_mode = if modes.is_empty() || compact {
        None
    } else {
        let id2 = id.clone();
        let (row, set) = choice(&modes, "Mode", move |m| {
            app::call(&id2, "set_hvac_mode", json!({"hvac_mode": m}));
        });
        frame.card.append(&row);
        Some(set)
    };

    // Setpoints: one target, or a low/high range in heat-and-cool mode. Steps
    // collect locally and go to Home Assistant once the clicking stops.
    let targets = widgets::hbox(10);
    targets.add_css_class("setpoints");
    if compact {
        targets.set_hexpand(true);
        readout.append(&targets);
    } else {
        frame.card.append(&targets);
    }
    let points = Rc::new(RefCell::new(Setpoints::default()));
    let debounce = Debounce::default();
    let mut steppers: Vec<(Point, gtk::Box, gtk::Label)> = Vec::new();
    for (title, point) in [
        ("Target", Point::Single),
        ("Heat to", Point::Low),
        ("Cool to", Point::High),
    ] {
        let b = widgets::vbox(2);
        b.set_hexpand(true);
        b.append(&widgets::label(title, "setpoint-title"));
        let row = widgets::hbox(4);
        let down = widgets::icon_button(
            "list-remove-symbolic",
            &format!("Lower {}", title.to_lowercase()),
        );
        let value = widgets::label("–", "setpoint");
        value.set_hexpand(true);
        value.set_xalign(0.5);
        let up = widgets::icon_button(
            "list-add-symbolic",
            &format!("Raise {}", title.to_lowercase()),
        );
        row.append(&down);
        row.append(&value);
        row.append(&up);
        b.append(&row);
        for (button, dir) in [(down, -1.0), (up, 1.0)] {
            let (points, value, debounce, id) =
                (points.clone(), value.clone(), debounce.clone(), id.clone());
            button.connect_clicked(move |_| {
                let shown = points.borrow_mut().step(point, dir);
                if let Some(v) = shown {
                    value.set_text(&temp(v));
                }
                let points = points.clone();
                let id = id.clone();
                debounce.run(900, move || {
                    let data = points.borrow_mut().send(point);
                    app::call(&id, "set_temperature", data);
                });
            });
        }
        targets.append(&b);
        steppers.push((point, b, value));
    }

    let fan_modes: Vec<(String, String)> = c.attributes["fan_modes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|m| (m.to_string(), sentence(m)))
        .collect();
    let set_fan = if fan_modes.len() > 1 && !compact {
        let row = widgets::hbox(10);
        let l = widgets::label("Fan", "setpoint-title");
        l.set_valign(gtk::Align::Center);
        row.append(&l);
        let id2 = id.clone();
        let (seg, set) = choice(&fan_modes, "Fan", move |m| {
            app::call(&id2, "set_fan_mode", json!({"fan_mode": m}));
        });
        seg.set_hexpand(true);
        row.append(&seg);
        frame.card.append(&row);
        Some(set)
    } else {
        None
    };

    let card = frame.card.clone();
    status(frame, device, show_area, move |home| {
        let Some(c) = entity(home, &id) else {
            return (String::new(), false, true);
        };
        let a = &c.attributes;
        current.set_text(
            &a["current_temperature"]
                .as_f64()
                .map(temp)
                .unwrap_or_else(|| "–".into()),
        );
        humidity.set_text(
            &a["current_humidity"]
                .as_f64()
                .map(|h| format!("{}% humidity", h.round()))
                .unwrap_or_default(),
        );
        let doing = a["hvac_action"].as_str().unwrap_or("");
        action.set_text(&match doing {
            "heating" => "Heating".to_string(),
            "cooling" => "Cooling".to_string(),
            "fan" => "Fan only".to_string(),
            "idle" => "Idle".to_string(),
            "off" | "" if c.state == "off" => "Off".to_string(),
            other => sentence(other),
        });
        set_class(
            &card,
            "heating",
            doing == "heating" || (doing.is_empty() && c.state == "heat"),
        );
        set_class(
            &card,
            "cooling",
            doing == "cooling" || (doing.is_empty() && c.state == "cool"),
        );
        set_class(&action, "heating", doing == "heating");
        set_class(&action, "cooling", doing == "cooling");
        if let Some(set) = &set_mode {
            set(&c.state);
        }
        if let Some(set) = &set_fan {
            set(a["fan_mode"].as_str().unwrap_or(""));
        }
        let range = a["target_temp_low"].is_number() && a["target_temp_high"].is_number();
        let one = a["temperature"].is_number();
        let mut p = points.borrow_mut();
        p.load(a);
        // The setpoints keep their place when the thermostat is off, dimmed.
        for (point, b, label) in &steppers {
            b.set_visible((*point == Point::Single) != range);
            label.set_text(&p.value(*point).map(temp).unwrap_or_else(|| "–".into()));
        }
        drop(p);
        let active = c.state != "off" && (one || range);
        targets.set_sensitive(active);
        set_class(&targets, "inactive", !active);
        let line = if range {
            format!(
                "{} · {}–{}",
                mode_label(&c.state),
                temp(a["target_temp_low"].as_f64().unwrap_or(0.0)),
                temp(a["target_temp_high"].as_f64().unwrap_or(0.0))
            )
        } else if one {
            format!(
                "{} · {}",
                mode_label(&c.state),
                temp(a["temperature"].as_f64().unwrap_or(0.0))
            )
        } else {
            mode_label(&c.state)
        };
        (line, c.state != "off", present::unavailable(c))
    });
    open_details_on_activate(device);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Point {
    Single,
    Low,
    High,
}

/// A thermostat's setpoints as shown, with the user's unsent steps kept until
/// they're sent so live updates don't undo them.
#[derive(Default)]
struct Setpoints {
    single: Option<f64>,
    low: Option<f64>,
    high: Option<f64>,
    step: f64,
    min: f64,
    max: f64,
    pending: bool,
}

impl Setpoints {
    fn load(&mut self, a: &Value) {
        self.step = a["target_temp_step"].as_f64().unwrap_or(1.0);
        self.min = a["min_temp"].as_f64().unwrap_or(40.0);
        self.max = a["max_temp"].as_f64().unwrap_or(95.0);
        if !self.pending {
            self.single = a["temperature"].as_f64();
            self.low = a["target_temp_low"].as_f64();
            self.high = a["target_temp_high"].as_f64();
        }
    }
    fn value(&self, p: Point) -> Option<f64> {
        match p {
            Point::Single => self.single,
            Point::Low => self.low,
            Point::High => self.high,
        }
    }
    fn step(&mut self, p: Point, dir: f64) -> Option<f64> {
        let (step, min, max) = (self.step.max(0.1), self.min, self.max);
        let slot = match p {
            Point::Single => &mut self.single,
            Point::Low => &mut self.low,
            Point::High => &mut self.high,
        };
        let next = ((slot.as_ref()? + dir * step) / step).round() * step;
        *slot = Some(next.clamp(min, max));
        self.pending = true;
        *slot
    }
    fn send(&mut self, p: Point) -> Value {
        self.pending = false;
        if p == Point::Single {
            json!({"temperature": self.single})
        } else {
            json!({"target_temp_low": self.low, "target_temp_high": self.high})
        }
    }
}

// ---------- Light ----------

/// Colors for the swatch: name and hue/saturation.
const COLORS: &[(&str, f64, f64)] = &[
    ("Red", 0.0, 100.0),
    ("Orange", 25.0, 100.0),
    ("Amber", 40.0, 100.0),
    ("Yellow", 55.0, 90.0),
    ("Lime", 90.0, 80.0),
    ("Green", 130.0, 80.0),
    ("Teal", 170.0, 80.0),
    ("Cyan", 190.0, 90.0),
    ("Blue", 220.0, 90.0),
    ("Indigo", 250.0, 80.0),
    ("Purple", 280.0, 80.0),
    ("Pink", 320.0, 70.0),
];

/// Whites by color temperature: name and kelvin.
const WHITES: &[(&str, u32)] = &[
    ("Candle", 2200),
    ("Warm", 2700),
    ("Soft", 3500),
    ("Neutral", 4500),
    ("Daylight", 6000),
];

fn hsv_hex(h: f64, s: f64) -> String {
    let rgb = gtk::hsv_to_rgb(h as f32 / 360.0, s as f32 / 100.0, 1.0);
    format!(
        "#{:02x}{:02x}{:02x}",
        (rgb.0 * 255.0) as u8,
        (rgb.1 * 255.0) as u8,
        (rgb.2 * 255.0) as u8
    )
}

/// An approximate color for a white of `kelvin`, for its swatch.
fn kelvin_hex(kelvin: u32) -> String {
    let t = ((kelvin as f64 - 2000.0) / 4500.0).clamp(0.0, 1.0);
    let lerp = |a: f64, b: f64| (a + (b - a) * t) as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        lerp(255.0, 235.0),
        lerp(176.0, 240.0),
        lerp(96.0, 255.0)
    )
}

fn light_colors(e: &Entity) -> (bool, Option<(u32, u32)>) {
    let modes: Vec<&str> = e.attributes["supported_color_modes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let color = modes
        .iter()
        .any(|m| matches!(*m, "hs" | "rgb" | "rgbw" | "rgbww" | "xy"));
    let white = modes.contains(&"color_temp").then(|| {
        (
            e.attributes["min_color_temp_kelvin"]
                .as_u64()
                .unwrap_or(2000) as u32,
            e.attributes["max_color_temp_kelvin"]
                .as_u64()
                .unwrap_or(6500) as u32,
        )
    });
    (color, white)
}

fn swatch(
    target: &Target,
    color: bool,
    white: Option<(u32, u32)>,
    current: Option<&Entity>,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    let body = widgets::vbox(10);
    body.set_margin_top(10);
    body.set_margin_bottom(10);
    body.set_margin_start(10);
    body.set_margin_end(10);
    let none = Value::Null;
    let attrs = current.map_or(&none, |c| &c.attributes);
    let hs = attrs["hs_color"]
        .as_array()
        .and_then(|v| Some((v.first()?.as_f64()?, v.get(1)?.as_f64()?)));
    let kelvin = attrs["color_temp_kelvin"].as_u64();
    let mode = attrs["color_mode"].as_str().unwrap_or("");
    let add = |grid: &gtk::Grid, i: i32, name: &str, hex: &str, selected: bool, data: Value| {
        let b = gtk::Button::new();
        b.add_css_class("swatch");
        widgets::paint(&b, hex);
        b.set_tooltip_text(Some(name));
        b.update_property(&[gtk::accessible::Property::Label(name)]);
        if selected {
            b.add_css_class("selected");
            b.set_child(Some(&gtk::Image::from_icon_name("object-select-symbolic")));
        }
        let (target, p) = (target.clone(), popover.clone());
        b.connect_clicked(move |_| {
            target.call("turn_on", data.clone());
            p.popdown();
        });
        grid.attach(&b, i % 6, i / 6, 1, 1);
    };
    if color {
        body.append(&widgets::label("Colors", "setpoint-title"));
        let grid = gtk::Grid::new();
        grid.set_row_spacing(8);
        grid.set_column_spacing(8);
        for (i, (name, h, s)) in COLORS.iter().enumerate() {
            let selected = mode == "hs"
                && hs.is_some_and(|(ch, cs)| (ch - h).abs() < 8.0 && (cs - s).abs() < 15.0);
            add(
                &grid,
                i as i32,
                name,
                &hsv_hex(*h, *s),
                selected,
                json!({"hs_color": [h, s]}),
            );
        }
        body.append(&grid);
    }
    if let Some((min, max)) = white {
        body.append(&widgets::label("Whites", "setpoint-title"));
        let grid = gtk::Grid::new();
        grid.set_row_spacing(8);
        grid.set_column_spacing(8);
        for (i, (name, k)) in WHITES.iter().enumerate() {
            let k = (*k).clamp(min, max);
            let selected =
                mode == "color_temp" && kelvin.is_some_and(|c| (c as i64 - k as i64).abs() < 250);
            add(
                &grid,
                i as i32,
                &format!("{name} ({k} K)"),
                &kelvin_hex(k),
                selected,
                json!({"color_temp_kelvin": k}),
            );
        }
        body.append(&grid);
    }
    popover.set_child(Some(&body));
    popover
}

fn light(frame: &Frame, device: &Device, home: &Home, show_area: bool, size: Size) {
    let Some(l) = device.primary(&home.entities) else {
        return;
    };
    if device.members.is_some() {
        frame.card.add_css_class("group-card");
    }
    let id = l.entity_id.clone();
    light_controls(
        frame,
        device,
        Target::Entity(id.clone()),
        vec![id],
        home,
        show_area,
        size.h == 1,
    );
}

pub use crate::device::ROOM_LIGHTS;

/// One card for every light in a room: on/off, one brightness and one color
/// for all of them.
/// The sizes a room's lights card comes in.
pub const ROOM_LIGHT_SIZES: [Size; 2] = [S24, S14];

pub fn room_lights(
    room: &str,
    lights: &[String],
    home: &Home,
    show_area: bool,
    size: Size,
) -> gtk::Widget {
    let device = Device {
        id: format!("{ROOM_LIGHTS}{room}"),
        name: if show_area {
            format!("{room} lights")
        } else {
            "All lights".into()
        },
        area: Some(room.to_string()),
        kind: Kind::Light,
        primary: None,
        entities: lights.to_vec(),
        maker: None,
        model: None,
        members: Some(lights.to_vec()),
        hidden: false,
        grouped: false,
        plugged: false,
    };
    let frame = frame(&device);
    frame.card.add_css_class("group-card");
    frame.card.add_css_class(&format!("size-h{}", size.h));
    frame
        .more
        .update_property(&[gtk::accessible::Property::Label(&format!(
            "Details for the {room} lights"
        ))]);
    if let Some(area) = home.areas.iter().find(|a| a.name == room) {
        light_controls(
            &frame,
            &device,
            Target::Area(area.id.clone()),
            lights.to_vec(),
            home,
            false,
            size.h == 1,
        );
    }
    finish(&frame, size);
    notify_one(frame.card.upcast_ref(), home);
    frame.card.upcast()
}

/// What a set of lights can do: dimming, colors, and a range of whites.
fn light_caps(lights: &[&Entity]) -> (bool, bool, Option<(u32, u32)>) {
    let dimmable = lights.iter().any(|l| present::brightness(l).is_some());
    let mut color = false;
    let mut white: Option<(u32, u32)> = None;
    for l in lights {
        let (c, w) = light_colors(l);
        color |= c;
        if let Some((lo, hi)) = w {
            white = Some(white.map_or((lo, hi), |(a, b)| (a.min(lo), b.max(hi))));
        }
    }
    (dimmable, color, white)
}

/// Switch, brightness and color for `target`, whose state is read from
/// `lights` (the light or group itself, or a room's lights).
fn light_controls(
    frame: &Frame,
    device: &Device,
    target: Target,
    lights: Vec<String>,
    home: &Home,
    show_area: bool,
    compact: bool,
) {
    let (sw, set_on) = target_switch(target.clone(), &device.name);
    frame.head_end.append(&sw);
    let found: Vec<&Entity> = lights.iter().filter_map(|id| entity(home, id)).collect();
    let (dimmable, color, white) = light_caps(&found);
    let row = widgets::hbox(8);
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1.0, 100.0, 1.0);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.set_size_request(60, -1);
    scale.update_property(&[gtk::accessible::Property::Label(&format!(
        "Brightness of {}",
        device.name
    ))]);
    let quiet = Rc::new(Cell::new(false));
    {
        let (target, quiet, debounce) = (target.clone(), quiet.clone(), Debounce::default());
        scale.connect_value_changed(move |s| {
            if quiet.get() {
                return;
            }
            let (target, v) = (target.clone(), s.value().round() as i64);
            debounce.run(300, move || {
                target.call("turn_on", json!({"brightness_pct": v}))
            });
        });
    }
    if dimmable {
        row.append(&scale);
    }
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("color-dot");
    dot.set_halign(gtk::Align::Center);
    dot.set_valign(gtk::Align::Center);
    let color_button = gtk::MenuButton::new();
    color_button.set_child(Some(&dot));
    color_button.add_css_class("color-button");
    color_button.set_valign(gtk::Align::Center);
    color_button.set_tooltip_text(Some("Color"));
    color_button.update_property(&[gtk::accessible::Property::Label(&format!(
        "Color of {}",
        device.name
    ))]);
    if color || white.is_some() {
        let (target, lights) = (target.clone(), lights.clone());
        color_button.set_create_popup_func(move |b| {
            let Some(app) = app::get() else { return };
            let home = app.home.borrow();
            // Mark the current color of the first light that's on.
            let current = lights
                .iter()
                .filter_map(|id| home.entities.get(id))
                .find(|l| l.state == "on");
            b.set_popover(Some(&swatch(&target, color, white, current)));
        });
        row.append(&color_button);
    }
    if (dimmable || color || white.is_some()) && !compact {
        frame.card.append(&row);
    }
    let dot_provider = gtk::CssProvider::new();
    #[allow(deprecated)]
    dot.style_context()
        .add_provider(&dot_provider, gtk::STYLE_PROVIDER_PRIORITY_USER);
    let members = device.members.clone();
    let watched: Vec<String> = lights
        .iter()
        .chain(members.iter().flatten())
        .cloned()
        .collect();
    let mut watched_device = device.clone();
    watched_device.entities = watched;
    status(frame, &watched_device, show_area, move |home| {
        let found: Vec<&Entity> = lights.iter().filter_map(|id| entity(home, id)).collect();
        if found.is_empty() {
            return (String::new(), false, true);
        }
        let lit: Vec<&&Entity> = found.iter().filter(|l| l.state == "on").collect();
        let on = !lit.is_empty();
        set_on(on);
        let level = (!lit.is_empty()).then(|| {
            lit.iter()
                .filter_map(|l| present::brightness(l))
                .sum::<f64>()
                / lit.len() as f64
        });
        quiet.set(true);
        scale.set_value(level.unwrap_or(0.0).max(1.0));
        quiet.set(false);
        let unavailable = found.iter().all(|l| present::unavailable(l));
        scale.set_sensitive(!unavailable);
        let rgb = lit
            .first()
            .and_then(|l| l.attributes["rgb_color"].as_array());
        match rgb.map(|c| c.iter().filter_map(Value::as_u64).collect::<Vec<_>>()) {
            Some(c) if c.len() == 3 => dot_provider.load_from_string(&format!(
                "* {{ background: rgb({},{},{}); }}",
                c[0], c[1], c[2]
            )),
            _ => dot_provider.load_from_string("* { background: alpha(currentColor, 0.25); }"),
        }
        let text = match &members {
            Some(members) => {
                let n = members.len();
                let m = members
                    .iter()
                    .filter_map(|id| home.entities.get(id))
                    .filter(|l| l.state == "on")
                    .count();
                let mut text = format!("{} · {m} on", present::plural(n, "light"));
                if let Some(level) = level.filter(|_| dimmable) {
                    text.push_str(&format!(" · {}%", level.round()));
                }
                text
            }
            None => present::state_text(found[0]),
        };
        (text, on, unavailable)
    });
    let s = sw.clone();
    set_activate(device, move || s.set_active(!s.is_active()));
}

// ---------- Robot ----------

fn robot_card(frame: &Frame, device: &Device, home: &Home, show_area: bool, size: Size) {
    let Some(r) = device.primary(&home.entities).cloned() else {
        return;
    };
    let members: Vec<&Entity> = device
        .entities
        .iter()
        .filter_map(|id| home.entities.get(id))
        .collect();
    if let Some(map) = present::robot_map(&r, members.iter().copied())
        && size.h == 4
    {
        frame.card.prepend(&viewer::thumbnail(
            &map.entity_id,
            &format!("{} map", device.name),
            160,
        ));
    }
    {
        let b = widgets::bar_button(icons::MAP, "Open: live map and controls");
        b.update_property(&[gtk::accessible::Property::Label(&format!(
            "Open {}: live map and controls",
            device.name
        ))]);
        let id = device.id.clone();
        b.connect_clicked(move |_| robot_window::open(&id));
        frame.head_end.append(&b);
    }
    let actions = home_features::robot_actions(&r);
    let main = [RobotAction::Start, RobotAction::Pause, RobotAction::Dock];
    let row = widgets::hbox(6);
    row.add_css_class("robot-actions");
    let shared = Rc::new(RefCell::new(r.clone()));
    let run = {
        let shared = shared.clone();
        move |action: RobotAction| run_robot(&shared.borrow(), action)
    };
    let run = Rc::new(run);
    for action in actions.iter().copied().filter(|a| main.contains(a)) {
        let b = widgets::icon_button(action.icon(), action.label(r.domain()));
        b.remove_css_class("flat");
        b.add_css_class("robot-action");
        b.update_property(&[gtk::accessible::Property::Label(action.label(r.domain()))]);
        let run = run.clone();
        b.connect_clicked(move |_| run(action));
        row.append(&b);
    }
    let extra: Vec<RobotAction> = actions
        .iter()
        .copied()
        .filter(|a| !main.contains(a))
        .collect();
    if !extra.is_empty() {
        let more = gtk::MenuButton::new();
        more.set_icon_name("view-more-horizontal-symbolic");
        more.add_css_class("robot-action");
        more.set_tooltip_text(Some("More commands"));
        more.update_property(&[gtk::accessible::Property::Label("More commands")]);
        let popover = gtk::Popover::new();
        popover.add_css_class("menu-popover");
        let items = widgets::vbox(0);
        for action in extra {
            let b = gtk::Button::new();
            let content = widgets::hbox(8);
            content.append(&gtk::Image::from_icon_name(action.icon()));
            content.append(&widgets::label(action.label(r.domain()), ""));
            b.set_child(Some(&content));
            b.add_css_class("flat");
            b.add_css_class("menu-item");
            let (run, p) = (run.clone(), popover.clone());
            b.connect_clicked(move |_| {
                p.popdown();
                run(action);
            });
            items.append(&b);
        }
        popover.set_child(Some(&items));
        more.set_popover(Some(&popover));
        row.append(&more);
    }
    if row.first_child().is_some() {
        frame.card.append(&row);
    }
    let battery = device
        .find(&home.entities, |e| {
            e.domain() == "sensor" && device::device_class(e) == "battery"
        })
        .map(|e| e.entity_id.clone());
    let id = r.entity_id.clone();
    status(frame, device, show_area, move |home| {
        let Some(r) = entity(home, &id) else {
            return (String::new(), false, true);
        };
        *shared.borrow_mut() = r.clone();
        let level = battery
            .as_ref()
            .and_then(|b| entity(home, b))
            .and_then(|e| e.state.parse::<f64>().ok())
            .or_else(|| r.attributes["battery_level"].as_f64());
        let line = match level {
            Some(l) => format!("{} · {}% battery", present::state_text(r), l.round()),
            None => present::state_text(r),
        };
        let busy = matches!(r.state.as_str(), "cleaning" | "mowing" | "returning");
        (line, busy, present::unavailable(r))
    });
    let id = device.id.clone();
    set_activate(device, move || robot_window::open(&id));
}

/// Send a robot command: choosing an area first for area cleaning, and the
/// safety check before a mower starts.
pub fn run_robot(entity: &Entity, action: RobotAction) {
    if action == RobotAction::CleanArea {
        choose_area(entity);
    } else if entity.domain() == "lawn_mower" && action == RobotAction::Start {
        confirm_mowing(entity);
    } else {
        robot(entity, action, None);
    }
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

// ---------- Media ----------

fn media(frame: &Frame, device: &Device, home: &Home, show_area: bool, size: Size) {
    let Some(m) = device.primary(&home.entities).cloned() else {
        return;
    };
    let id = m.entity_id.clone();
    let f = features(&m);
    let compact = size.h == 1;
    let play = (f & (1 | 16384) != 0).then(|| {
        let b = widgets::icon_button("media-playback-start-symbolic", "Play or pause");
        b.update_property(&[gtk::accessible::Property::Label("Play or pause")]);
        let id = id.clone();
        b.connect_clicked(move |_| app::call(&id, "media_play_pause", json!({})));
        b
    });
    if compact && let Some(b) = &play {
        frame.head_end.append(b);
    }
    let power = (f & (128 | 256) != 0).then(|| {
        let (sw, set) = switch(&id, &format!("{} power", device.name));
        frame.head_end.append(&sw);
        set
    });
    // At 2x4 one row: play, what's playing, volume. The source is in details.
    let row = widgets::hbox(8);
    if let Some(b) = play.as_ref().filter(|_| !compact) {
        row.append(b);
    }
    let playing = widgets::label("", "now-playing");
    playing.set_ellipsize(gtk::pango::EllipsizeMode::End);
    playing.set_hexpand(true);
    playing.set_width_chars(6);
    row.append(&playing);
    let volume = (f & 4 != 0).then(|| {
        let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        s.set_draw_value(false);
        s.set_size_request(90, -1);
        s.update_property(&[gtk::accessible::Property::Label(&format!(
            "Volume of {}",
            device.name
        ))]);
        let quiet = Rc::new(Cell::new(false));
        let (id, q, debounce) = (id.clone(), quiet.clone(), Debounce::default());
        s.connect_value_changed(move |s| {
            if q.get() {
                return;
            }
            let (id, v) = (id.clone(), s.value() / 100.0);
            debounce.run(300, move || {
                app::call(&id, "volume_set", json!({"volume_level": v}))
            });
        });
        row.append(&gtk::Image::from_icon_name("audio-speakers-symbolic"));
        row.append(&s);
        (s, quiet)
    });
    if !compact {
        frame.card.append(&row);
    }
    status(frame, device, show_area, move |home| {
        let Some(m) = entity(home, &id) else {
            return (String::new(), false, true);
        };
        let a = &m.attributes;
        let title = [
            a["media_title"].as_str(),
            a["media_artist"].as_str(),
            a["app_name"].as_str(),
        ]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .take(2)
        .collect::<Vec<_>>()
        .join(" · ");
        playing.set_text(if title.is_empty() {
            "Nothing playing"
        } else {
            &title
        });
        if let Some(set) = &power {
            set(m.state != "off");
        }
        if let Some(b) = &play {
            b.set_icon_name(if m.state == "playing" {
                "media-playback-pause-symbolic"
            } else {
                "media-playback-start-symbolic"
            });
        }
        if let Some((s, q)) = &volume {
            q.set(true);
            s.set_value(a["volume_level"].as_f64().unwrap_or(0.0) * 100.0);
            q.set(false);
            s.set_sensitive(a["volume_level"].is_f64());
        }
        let line = if compact && !title.is_empty() {
            format!("{} · {title}", sentence(&m.state))
        } else {
            sentence(&m.state)
        };
        (
            line,
            matches!(m.state.as_str(), "playing" | "on"),
            present::unavailable(m),
        )
    });
    open_details_on_activate(device);
}

// ---------- Fan and air purifier ----------

/// Up to three readings side by side: value over name.
fn readings_row(card: &gtk::Box, ids: Vec<String>) -> Rc<dyn Fn(&Home)> {
    let row = widgets::hbox(10);
    row.add_css_class("readings");
    row.set_homogeneous(true);
    let mut labels = Vec::new();
    for id in ids.iter().take(3) {
        let b = widgets::vbox(1);
        let k = widgets::label("", "reading-name");
        k.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let v = widgets::label("", "reading-value");
        v.set_ellipsize(gtk::pango::EllipsizeMode::End);
        b.append(&v);
        b.append(&k);
        row.append(&b);
        labels.push((id.clone(), k, v));
    }
    if !ids.is_empty() {
        card.append(&row);
    }
    Rc::new(move |home: &Home| {
        for (id, k, v) in &labels {
            if let Some(e) = entity(home, id) {
                k.set_text(&present::short_name(e));
                v.set_text(&present::state_text(e));
            }
        }
    })
}

/// Readings as text for a status line: "PM2.5 1 µg/m³ · Excellent".
fn readings_text(home: &Home, ids: &[String]) -> String {
    ids.iter()
        .filter_map(|id| entity(home, id))
        .map(|e| {
            let value = present::state_text(e);
            if e.attributes["unit_of_measurement"].is_string() {
                format!("{} {value}", present::short_name(e))
            } else {
                value
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

fn fan(frame: &Frame, device: &Device, home: &Home, show_area: bool, size: Size) {
    let Some(f) = device.primary(&home.entities).cloned() else {
        return;
    };
    let id = f.entity_id.clone();
    let (sw, set_on) = switch(&id, &device.name);
    frame.head_end.append(&sw);
    let presets: Vec<(String, String)> = f.attributes["preset_modes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|m| (m.to_string(), sentence(m)))
        .collect();
    let set_preset = (features(&f) & 8 != 0 && !presets.is_empty() && size.h > 1).then(|| {
        let id2 = id.clone();
        let (row, set) = choice(&presets, "Mode", move |m| {
            app::call(&id2, "set_preset_mode", json!({"preset_mode": m}))
        });
        frame.card.append(&row);
        set
    });
    let readings: Vec<String> = device
        .readings(&home.entities, 2)
        .iter()
        .map(|e| e.entity_id.clone())
        .collect();
    status(frame, device, show_area, move |home| {
        let Some(f) = entity(home, &id) else {
            return (String::new(), false, true);
        };
        set_on(f.state == "on");
        if let Some(set) = &set_preset {
            set(f.attributes["preset_mode"].as_str().unwrap_or(""));
        }
        let mut line = match (f.state.as_str(), f.attributes["preset_mode"].as_str()) {
            ("on", Some(p)) => format!("On · {}", sentence(p)),
            _ => present::state_text(f),
        };
        let extra = readings_text(home, &readings);
        if !extra.is_empty() {
            line = format!("{line} · {extra}");
        }
        (line, f.state == "on", present::unavailable(f))
    });
    let s = sw.clone();
    set_activate(device, move || s.set_active(!s.is_active()));
}

// ---------- Security ----------

fn security(frame: &Frame, device: &Device, home: &Home, show_area: bool) {
    let Some(p) = device.primary(&home.entities).cloned() else {
        return;
    };
    let id = p.entity_id.clone();
    let f = features(&p);
    let mut modes = Vec::new();
    if f & 1 != 0 {
        modes.push(("armed_home".to_string(), "Home".to_string()));
    }
    if f & 2 != 0 {
        modes.push(("armed_away".to_string(), "Away".to_string()));
    }
    if f & 4 != 0 {
        modes.push(("armed_night".to_string(), "Night".to_string()));
    }
    modes.push(("disarmed".to_string(), "Off".to_string()));
    let refresh = Rc::new(RefCell::new(None::<Rc<dyn Fn()>>));
    let (id2, name, refresh2) = (id.clone(), device.name.clone(), refresh.clone());
    let (row, set) = choice(&modes, "Security mode", move |m| {
        let service = match m {
            "armed_home" => "alarm_arm_home",
            "armed_away" => "alarm_arm_away",
            "armed_night" => "alarm_arm_night",
            _ => "alarm_disarm",
        };
        if service == "alarm_disarm" {
            // Turning security off needs a second, deliberate click.
            if let Some(f) = refresh2.borrow().as_ref() {
                f();
            }
            let id = id2.clone();
            app::toast_action(&format!("Turn off {name}?"), "Turn off", move || {
                app::call(&id, "alarm_disarm", json!({}));
            });
        } else {
            app::call(&id2, service, json!({}));
        }
    });
    frame.card.append(&row);
    let set2 = set.clone();
    let id3 = id.clone();
    *refresh.borrow_mut() = Some(Rc::new(move || {
        if let Some(app) = app::get()
            && let Some(e) = app.home.borrow().entities.get(&id3)
        {
            set2(&e.state);
        }
    }));
    status(frame, device, show_area, move |home| {
        let Some(p) = entity(home, &id) else {
            return (String::new(), false, true);
        };
        set(&p.state);
        (
            sentence(&p.state),
            p.state.starts_with("armed"),
            present::unavailable(p),
        )
    });
    open_details_on_activate(device);
}

// ---------- Lock, cover, switches, sensors ----------

fn lock(frame: &Frame, device: &Device, home: &Home, show_area: bool) {
    let Some(l) = device.primary(&home.entities).cloned() else {
        return;
    };
    let id = l.entity_id.clone();
    let id2 = id.clone();
    let button = widgets::two_click("Unlock", "Unlock?", move || {
        app::call(&id2, "unlock", json!({}))
    });
    button.remove_css_class("destructive-action");
    let lock_button = gtk::Button::with_label("Lock");
    let id3 = id.clone();
    lock_button.connect_clicked(move |_| app::call(&id3, "lock", json!({})));
    frame.head_end.append(&button);
    frame.head_end.append(&lock_button);
    status(frame, device, show_area, move |home| {
        let Some(l) = entity(home, &id) else {
            return (String::new(), false, true);
        };
        let locked = l.state == "locked";
        button.set_visible(locked);
        lock_button.set_visible(!locked);
        (present::state_text(l), !locked, present::unavailable(l))
    });
    open_details_on_activate(device);
}

fn cover(frame: &Frame, device: &Device, home: &Home, show_area: bool) {
    let Some(c) = device.primary(&home.entities).cloned() else {
        return;
    };
    let id = c.entity_id.clone();
    let row = widgets::hbox(6);
    for (label, service, icon) in present::cover_actions(&c) {
        let b = widgets::icon_button(icon, label);
        let id = id.clone();
        b.connect_clicked(move |_| app::call(&id, service, json!({})));
        row.append(&b);
    }
    frame.head_end.append(&row);
    status(frame, device, show_area, move |home| {
        match entity(home, &id) {
            Some(c) => (
                present::state_text(c),
                c.state == "open",
                present::unavailable(c),
            ),
            None => (String::new(), false, true),
        }
    });
    open_details_on_activate(device);
}

fn toggles(device: &Device, home: &Home) -> Vec<Entity> {
    device
        .entities
        .iter()
        .filter_map(|id| home.entities.get(id))
        .filter(|e| matches!(e.domain(), "switch" | "input_boolean") && device::visible(e, false))
        .cloned()
        .collect()
}

fn buttons(device: &Device, home: &Home) -> Vec<Entity> {
    device
        .entities
        .iter()
        .filter_map(|id| home.entities.get(id))
        .filter(|e| matches!(e.domain(), "button" | "input_button") && device::visible(e, false))
        .cloned()
        .collect()
}

fn switches(frame: &Frame, device: &Device, home: &Home, show_area: bool, size: Size) {
    let toggles = toggles(device, home);
    let mut setters = Vec::new();
    // One toggle sits in the header; with more, a 2x4 card shows two in a row.
    let row = widgets::hbox(12);
    row.add_css_class("toggle-row");
    if toggles.len() == 1 {
        let e = &toggles[0];
        let (sw, set) = switch(&e.entity_id, &device.name);
        frame.head_end.append(&sw);
        setters.push((e.entity_id.clone(), set));
    } else if size.h > 1 {
        for e in toggles.iter().take(2) {
            let item = widgets::hbox(8);
            item.set_hexpand(true);
            let l = widgets::label(&present::short_name(e), "");
            l.set_hexpand(true);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            l.set_width_chars(4);
            item.append(&l);
            let (sw, set) = switch(&e.entity_id, &e.name());
            item.append(&sw);
            row.append(&item);
            setters.push((e.entity_id.clone(), set));
        }
        if toggles.len() > 2 {
            let more = gtk::Button::with_label(&format!("+{}", toggles.len() - 2));
            more.add_css_class("flat");
            more.set_tooltip_text(Some("The rest are in details"));
            let id = device.id.clone();
            more.connect_clicked(move |_| details::open(&id));
            row.append(&more);
        }
    }
    // Devices whose only controls are buttons (restart, identify) get them here.
    if toggles.is_empty() && size.h > 1 {
        for e in buttons(device, home).iter().take(2) {
            let b = gtk::Button::with_label(&present::short_name(e));
            b.set_hexpand(true);
            let id = e.entity_id.clone();
            b.connect_clicked(move |_| app::call(&id, "press", json!({})));
            row.append(&b);
        }
    }
    if row.first_child().is_some() {
        frame.card.append(&row);
    }
    let first = toggles.first().map(|e| e.entity_id.clone());
    let all: Vec<String> = toggles.iter().map(|e| e.entity_id.clone()).collect();
    status(frame, device, show_area, move |home| {
        for (id, set) in &setters {
            set(entity(home, id).is_some_and(|e| e.state == "on"));
        }
        let on_count = all
            .iter()
            .filter(|id| entity(home, id).is_some_and(|e| e.state == "on"))
            .count();
        let line = match all.len() {
            0 => String::new(),
            1 => first
                .as_ref()
                .and_then(|id| entity(home, id))
                .map(present::state_text)
                .unwrap_or_default(),
            n => format!("{on_count} of {n} on"),
        };
        let unavailable = first
            .as_ref()
            .and_then(|id| entity(home, id))
            .is_some_and(present::unavailable);
        (line, on_count > 0, unavailable)
    });
    if toggles.len() == 1 {
        let id = toggles[0].entity_id.clone();
        set_activate(device, move || {
            let on = app::get().is_some_and(|a| {
                a.home
                    .borrow()
                    .entities
                    .get(&id)
                    .is_some_and(|e| e.state == "on")
            });
            app::call(&id, present::toggle_service(!on), json!({}));
        });
    } else {
        open_details_on_activate(device);
    }
}

fn sensors(frame: &Frame, device: &Device, home: &Home, show_area: bool, size: Size) {
    let readings: Vec<String> = device
        .readings(&home.entities, 3)
        .iter()
        .map(|e| e.entity_id.clone())
        .collect();
    // A 1x4 card says its main reading in the status line.
    let compact = size.h == 1;
    let fill = if compact {
        Rc::new(|_: &Home| {}) as Rc<dyn Fn(&Home)>
    } else {
        readings_row(&frame.card, readings.clone())
    };
    let count = device
        .entities
        .iter()
        .filter(|id| {
            home.entities
                .get(*id)
                .is_some_and(|e| device::visible(e, false))
        })
        .count();
    status(frame, device, show_area, move |home| {
        fill(home);
        let line = if compact {
            readings_text(home, &readings[..readings.len().min(1)])
        } else if count > readings.len() {
            format!("{} more in details", count - readings.len())
        } else {
            String::new()
        };
        let unavailable = !readings.is_empty()
            && readings
                .iter()
                .all(|id| entity(home, id).is_none_or(present::unavailable));
        (line, false, unavailable)
    });
    open_details_on_activate(device);
}

/// The sizes a scene tile comes in.
pub const SCENE_SIZES: [Size; 3] = [S12, S22, S14];

/// A scene or script as a colored tile: tap to run it. The color stays the
/// same for each scene.
pub fn scene_tile(id: &str, home: &Home, size: Size) -> gtk::Widget {
    let name = home
        .entities
        .get(id)
        .map(present::short_name)
        .unwrap_or_else(|| id.to_string());
    let script = id.starts_with("script.");
    let tile: gtk::Box = glib::Object::builder()
        .property("orientation", gtk::Orientation::Vertical)
        .property("accessible-role", gtk::AccessibleRole::Button)
        .build();
    tile.add_css_class("scene-tile");
    tile.set_focusable(true);
    let hue = id
        .bytes()
        .fold(0u32, |h, c| h.wrapping_mul(31).wrapping_add(c as u32))
        % 8;
    tile.add_css_class(&format!("scene-color-{hue}"));
    let icon = gtk::Image::from_icon_name(if script {
        "system-run-symbolic"
    } else {
        "applications-graphics-symbolic"
    });
    let label = widgets::label(&name, "scene-name");
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let more = widgets::bar_button("view-more-symbolic", "Name, room and more (Shift+F10)");
    more.add_css_class("scene-more");
    more.set_valign(gtk::Align::Center);
    more.update_property(&[gtk::accessible::Property::Label(&format!(
        "Details for {name}"
    ))]);
    {
        let id = id.to_string();
        more.connect_clicked(move |_| details::open(&id));
    }
    if size.h >= 2 {
        let top = widgets::hbox(6);
        icon.set_pixel_size(24);
        icon.set_hexpand(true);
        icon.set_halign(gtk::Align::Start);
        top.append(&icon);
        top.append(&more);
        tile.append(&top);
        label.set_wrap(true);
        label.set_lines(2);
        label.set_valign(gtk::Align::End);
        label.set_vexpand(true);
        tile.append(&label);
    } else {
        // One row: the name gets two lines beside the icon.
        tile.add_css_class("compact");
        let body = widgets::hbox(8);
        body.set_vexpand(true);
        icon.set_valign(gtk::Align::Center);
        body.append(&icon);
        label.set_wrap(true);
        label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        label.set_lines(2);
        label.set_xalign(0.0);
        label.set_valign(gtk::Align::Center);
        label.set_hexpand(true);
        body.append(&label);
        body.append(&more);
        tile.append(&body);
    }
    tile.set_tooltip_text(Some(&format!(
        "Run {name}. Right-click for its name and room."
    )));
    tile.update_property(&[gtk::accessible::Property::Label(&format!("Run {name}"))]);
    let run = {
        let id = id.to_string();
        move || {
            app::call(&id, "turn_on", json!({}));
            app::toast("Done.");
        }
    };
    // A click runs it (the ⋯ button keeps its own clicks); right-click, a long
    // press, the Menu key or Shift+F10 open its details.
    let click = gtk::GestureClick::new();
    click.set_button(0);
    {
        let (run, id) = (run.clone(), id.to_string());
        click.connect_released(move |g, _, _, _| match g.current_button() {
            gdk::BUTTON_PRIMARY => run(),
            gdk::BUTTON_SECONDARY => details::open(&id),
            _ => {}
        });
    }
    tile.add_controller(click);
    let press = gtk::GestureLongPress::new();
    {
        let id = id.to_string();
        press.connect_pressed(move |_, _, _| details::open(&id));
    }
    tile.add_controller(press);
    let keys = gtk::EventControllerKey::new();
    {
        let (id, me) = (id.to_string(), tile.downgrade());
        keys.connect_key_pressed(move |_, key, _, mods| {
            if !me.upgrade().is_some_and(|t| t.has_focus()) {
                return glib::Propagation::Proceed;
            }
            if key == gdk::Key::Menu
                || (key == gdk::Key::F10 && mods.contains(gdk::ModifierType::SHIFT_MASK))
            {
                details::open(&id);
            } else if matches!(key, gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space) {
                run();
            } else {
                return glib::Propagation::Proceed;
            }
            glib::Propagation::Stop
        });
    }
    tile.add_controller(keys);
    tile.upcast()
}

/// Builds any card on a page by ID at a size: a device, a room's lights or a
/// scene. Grids use it to rebuild a card whose size changes.
pub fn maker(devices: Vec<Device>, show_area: bool) -> grid::Maker {
    Rc::new(move |id: &str, size: Size| -> gtk::Widget {
        let Some(app) = app::get() else {
            return gtk::Box::new(gtk::Orientation::Vertical, 0).upcast();
        };
        let home = app.home.borrow();
        if let Some(room) = id.strip_prefix(ROOM_LIGHTS) {
            let lights = home
                .room_light_sets()
                .into_iter()
                .find(|(r, _)| r == room)
                .map(|(_, l)| l)
                .unwrap_or_default();
            return room_lights(room, &lights, &home, show_area, size);
        }
        if id.starts_with("scene.") || id.starts_with("script.") {
            return scene_tile(id, &home, size);
        }
        match devices.iter().find(|d| d.id == id) {
            Some(d) => card(d, &home, show_area, size),
            None => gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
        }
    })
}

/// Add `id` to `grid` at its saved or default size.
pub fn add_to(grid: &grid::CardGrid, id: &str, options: Vec<Size>) {
    let size = grid::saved_size(id, &options).unwrap_or(options[0]);
    grid.add(id, size, options);
}
