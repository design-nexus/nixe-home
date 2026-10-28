//! A device's details: everything it exposes that its card leaves out,
//! sorted into Controls, Readings, Settings and Diagnostics, each with the
//! control its type calls for.
use crate::{
    app,
    cards::{self, Debounce},
    connection,
    device::{self, Device, Section},
    ha::{self, Entity},
    light_groups, prefs, present, viewer,
};
use gtk::{glib, prelude::*};
use nyx_ui::widgets;
use serde_json::{Value, json};
use std::{cell::Cell, rc::Rc};

pub fn open(device_id: &str) {
    if let Some(room) = device_id.strip_prefix(cards::ROOM_LIGHTS) {
        open_room(room);
        return;
    }
    if device_id.starts_with("scene.") || device_id.starts_with("script.") {
        open_scene(device_id);
        return;
    }
    let Some(app) = app::get() else { return };
    let (device, members, base) = {
        let home = app.home.borrow();
        let Some(device) = home
            .device_list(true)
            .into_iter()
            .find(|d| d.id == device_id)
        else {
            app::toast("That device is no longer in Home Assistant.");
            return;
        };
        let members: Vec<Entity> = device
            .entities
            .iter()
            .filter_map(|id| home.entities.get(id).cloned())
            .collect();
        (device, members, home.base.clone())
    };
    let (dialog, card) = widgets::dialog(&device.name, 580);
    dialog.set_default_height(680);
    let about: Vec<String> = [
        device.area.clone(),
        device.maker.clone(),
        device.model.clone(),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.trim().is_empty())
    .collect();
    if !about.is_empty() {
        card.append(&widgets::label(&about.join(" · "), "dim"));
    }
    let actions = widgets::hbox(8);
    let favorite = prefs::get().favorite_devices.contains(&device.id);
    let star = widgets::labeled_button(
        if favorite {
            "starred-symbolic"
        } else {
            "non-starred-symbolic"
        },
        if favorite {
            "On Overview"
        } else {
            "Add to Overview"
        },
    );
    {
        let id = device.id.clone();
        star.connect_clicked(move |b| {
            let added = toggle_favorite(&id);
            if let Some(content) = b.child().and_downcast::<gtk::Box>() {
                if let Some(icon) = content.first_child().and_downcast::<gtk::Image>() {
                    icon.set_icon_name(Some(if added {
                        "starred-symbolic"
                    } else {
                        "non-starred-symbolic"
                    }));
                }
                if let Some(label) = content.last_child().and_downcast::<gtk::Label>() {
                    label.set_text(if added {
                        "On Overview"
                    } else {
                        "Add to Overview"
                    });
                }
            }
        });
    }
    actions.append(&star);
    if !device.id.starts_with("entity:") && !base.is_empty() {
        let open = widgets::labeled_button("network-server-symbolic", "Open in Home Assistant");
        let url = format!("{base}/config/devices/device/{}", device.id);
        let d = dialog.clone();
        open.connect_clicked(move |_| {
            gtk::UriLauncher::new(&url).launch(Some(&d), None::<&gtk::gio::Cancellable>, |_| {});
        });
        actions.append(&open);
    }
    card.append(&actions);
    {
        let home = app.home.borrow();
        card.append(&settings(&device, &home, "DEVICE"));
    }

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some(&format!("Search {} items", members.len())));
    search.update_property(&[gtk::accessible::Property::Label("Search this device")]);
    if members.len() > 10 {
        card.append(&search);
    }

    let body = widgets::vbox(6);
    let mut rows: Vec<(gtk::Widget, String)> = Vec::new();
    let mut expanders = Vec::new();
    let mut watched: Vec<String> = device.entities.clone();
    if let Some(lights) = &device.members {
        let home = app.home.borrow();
        let list = light_rows(lights, &home);
        body.append(&widgets::label(
            &format!("LIGHTS IN THIS GROUP · {}", lights.len()),
            "group-title",
        ));
        body.append(&list);
        watched.extend(lights.iter().cloned());
    }
    for section in [
        Section::Controls,
        Section::Readings,
        Section::Settings,
        Section::Diagnostics,
    ] {
        let mut items: Vec<&Entity> = members
            .iter()
            .filter(|e| device::section(e) == Some(section))
            .collect();
        if items.is_empty() {
            continue;
        }
        items.sort_by_key(|e| present::short_name(e).to_lowercase());
        let list = widgets::vbox(0);
        list.add_css_class("group-list");
        for e in items.iter() {
            let row = row(e);
            rows.push((
                row.clone().upcast(),
                format!("{} {}", e.name(), e.entity_id).to_lowercase(),
            ));
            list.append(&row);
        }
        let title = format!("{} · {}", section.title().to_uppercase(), items.len());
        if matches!(section, Section::Settings | Section::Diagnostics) && items.len() > 6 {
            let expander = gtk::Expander::new(Some(&title));
            expander.add_css_class("details-expander");
            expander.set_child(Some(&list));
            body.append(&expander);
            expanders.push(expander);
        } else {
            body.append(&widgets::label(&title, "group-title"));
            body.append(&list);
        }
    }
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .min_content_height(320)
        .child(&body)
        .build();
    card.append(&scroll);
    let close = gtk::Button::with_label("Close");
    close.set_halign(gtk::Align::End);
    let d = dialog.clone();
    close.connect_clicked(move |_| d.close());
    card.append(&close);
    search.connect_search_changed(move |s| {
        let q = s.text().trim().to_lowercase();
        for (row, text) in &rows {
            row.set_visible(q.is_empty() || text.contains(&q));
        }
        for e in &expanders {
            e.set_expanded(!q.is_empty());
        }
    });
    // Fill controls in from the current state.
    {
        let home = app.home.borrow();
        cards::notify(&home, Some(&watched.into_iter().collect()));
    }
    dialog.present();
}

/// Name, room and hiding, and for a light group its lights.
fn settings(device: &Device, home: &app::Home, title: &str) -> gtk::Box {
    let list = widgets::vbox(0);
    list.add_css_class("group-list");
    list.add_css_class("device-settings");

    let name = gtk::Entry::new();
    name.set_text(&device.name);
    name.set_valign(gtk::Align::Center);
    name.set_width_chars(22);
    name.update_property(&[gtk::accessible::Property::Label("Name")]);
    let saved = Rc::new(std::cell::RefCell::new(device.name.clone()));
    let save_name = {
        let (id, saved) = (device.id.clone(), saved.clone());
        move |entry: &gtk::Entry| {
            let text = entry.text().trim().to_string();
            if text.is_empty() || *saved.borrow() == text {
                return;
            }
            *saved.borrow_mut() = text.clone();
            update_registry(&id, json!({"name": text}));
        }
    };
    name.connect_activate(save_name.clone());
    let focus = gtk::EventControllerFocus::new();
    {
        let name = name.clone();
        focus.connect_leave(move |_| save_name(&name));
    }
    name.add_controller(focus);
    list.append(&widgets::row("Name", "", Some(name.upcast_ref())));

    let mut rooms: Vec<String> = vec!["No room".into()];
    rooms.extend(home.areas.iter().map(|a| a.name.clone()));
    let ids: Vec<Option<String>> = std::iter::once(None)
        .chain(home.areas.iter().map(|a| Some(a.id.clone())))
        .collect();
    let refs: Vec<&str> = rooms.iter().map(String::as_str).collect();
    let room = gtk::DropDown::from_strings(&refs);
    room.set_valign(gtk::Align::Center);
    room.update_property(&[gtk::accessible::Property::Label("Room")]);
    let current = device
        .area
        .as_ref()
        .and_then(|a| rooms.iter().position(|r| r == a))
        .unwrap_or(0);
    room.set_selected(current as u32);
    {
        let id = device.id.clone();
        room.connect_selected_notify(move |d| {
            if let Some(area) = ids.get(d.selected() as usize) {
                update_registry(&id, json!({"area_id": area}));
            }
        });
    }
    list.append(&widgets::row(
        "Room",
        "The area in Home Assistant",
        Some(room.upcast_ref()),
    ));

    let hide = gtk::Switch::new();
    hide.set_valign(gtk::Align::Center);
    hide.set_active(device.hidden);
    hide.update_property(&[gtk::accessible::Property::Label("Hide in Nyx Home")]);
    {
        let id = device.id.clone();
        hide.connect_active_notify(move |s| set_hidden(&id, s.is_active()));
    }
    list.append(&widgets::row(
        "Hide in Nyx Home",
        "Still in Home Assistant. Show every device (the eye button) brings it back.",
        Some(hide.upcast_ref()),
    ));

    if device.members.is_some() {
        list.append(&collapse_row(&device.id));
    }
    if let Some(entry) = device.group_entry(&home.entities) {
        let buttons = widgets::hbox(8);
        let edit = gtk::Button::with_label("Choose lights…");
        {
            let id = device.id.clone();
            edit.connect_clicked(move |_| light_groups::edit(&id));
        }
        buttons.append(&edit);
        let (name, row) = (device.name.clone(), buttons.downgrade());
        let delete = widgets::two_click("Delete group", "Delete it?", move || {
            light_groups::delete(&entry, &name);
            if let Some(window) = row
                .upgrade()
                .and_then(|r| r.root())
                .and_downcast::<gtk::Window>()
            {
                window.close();
            }
        });
        buttons.append(&delete);
        list.append(&widgets::row(
            "Light group",
            "Made in Home Assistant; it keeps its lights in step",
            Some(buttons.upcast_ref()),
        ));
    }
    let section = widgets::vbox(6);
    section.append(&widgets::label(title, "group-title"));
    section.append(&list);
    section
}

/// Save registry `changes` (`name`, `area_id`) for a device, or for an
/// entity on its own, then load the registry again.
fn update_registry(device_id: &str, changes: Value) {
    let mut command = match device_id.strip_prefix("entity:") {
        Some(entity) => json!({"type": "config/entity_registry/update", "entity_id": entity}),
        None => {
            let mut c = json!({"type": "config/device_registry/update", "device_id": device_id});
            // Devices keep their own name; the user's goes in name_by_user.
            if let Some(name) = changes.get("name") {
                c["name_by_user"] = name.clone();
            }
            c
        }
    };
    if let (Some(map), Value::Object(target)) = (changes.as_object(), &mut command) {
        for (k, v) in map {
            if !(k == "name" && target.contains_key("device_id")) {
                target.insert(k.clone(), v.clone());
            }
        }
    }
    app::ha_task(
        move |base, token| async move { ha::ws_command(&base, &token, command).await },
        |_| {
            app::toast("Saved.");
            connection::reconnect();
        },
    );
}

/// Rows for a group's lights, each with its own control.
fn light_rows(lights: &[String], home: &app::Home) -> gtk::Box {
    let list = widgets::vbox(0);
    list.add_css_class("group-list");
    for id in lights {
        if let Some(e) = home.entities.get(id) {
            list.append(&row(e));
        }
    }
    list
}

/// "Hide its lights in Nyx Home" for a light group or a room's lights.
fn collapse_row(group_id: &str) -> gtk::Box {
    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Center);
    switch.set_active(prefs::get().collapsed_groups.iter().any(|g| g == group_id));
    switch.update_property(&[gtk::accessible::Property::Label("Hide its lights")]);
    let id = group_id.to_string();
    switch.connect_active_notify(move |s| {
        let on = s.is_active();
        prefs::update(|p| {
            p.collapsed_groups.retain(|g| *g != id);
            if on {
                p.collapsed_groups.push(id.clone());
            }
        });
        app::rebuild();
    });
    widgets::row(
        "Hide its lights",
        "Only the group shows in Nyx Home; its lights stay here and in Home Assistant.",
        Some(switch.upcast_ref()),
    )
}

/// The details of a room's lights as one: hide them behind the group, use
/// each one, or save them as a light group in Home Assistant.
fn open_room(room: &str) {
    let Some(app) = app::get() else { return };
    let home = app.home.borrow();
    let Some((_, lights)) = home.room_light_sets().into_iter().find(|(r, _)| r == room) else {
        app::toast("That room no longer has two or more lights.");
        return;
    };
    let (dialog, card) = widgets::dialog(&format!("{room} lights"), 540);
    let note = widgets::label(
        &format!("Every light in {room} as one: one switch, one brightness and one color."),
        "dim",
    );
    note.set_wrap(true);
    card.append(&note);
    let options = widgets::vbox(0);
    options.add_css_class("group-list");
    options.append(&collapse_row(&format!("{}{room}", cards::ROOM_LIGHTS)));
    let save = gtk::Button::with_label("Make group…");
    save.set_valign(gtk::Align::Center);
    {
        let (room, d) = (room.to_string(), dialog.clone());
        save.connect_clicked(move |_| {
            d.close();
            light_groups::create_for_room(&room);
        });
    }
    options.append(&widgets::row(
        "Save as a light group",
        "Make it a group in Home Assistant, to use in its app, automations and voice assistants",
        Some(save.upcast_ref()),
    ));
    card.append(&options);
    card.append(&widgets::label(
        &format!("LIGHTS · {}", lights.len()),
        "group-title",
    ));
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(420)
        .child(&light_rows(&lights, &home))
        .build();
    card.append(&scroll);
    let close = gtk::Button::with_label("Close");
    close.set_halign(gtk::Align::End);
    let d = dialog.clone();
    close.connect_clicked(move |_| d.close());
    card.append(&close);
    cards::notify(&home, Some(&lights.iter().cloned().collect()));
    drop(home);
    dialog.present();
}

/// A scene or script: run it, rename it, put it in a room, hide it, or open
/// it in Home Assistant to change what it does.
fn open_scene(id: &str) {
    let Some(app) = app::get() else { return };
    let home = app.home.borrow();
    let Some(e) = home.entities.get(id).cloned() else {
        app::toast("That scene is no longer in Home Assistant.");
        return;
    };
    let script = id.starts_with("script.");
    let device = Device {
        id: format!("entity:{id}"),
        name: present::short_name(&e),
        area: e.area_name.clone(),
        kind: device::Kind::Switch,
        primary: Some(id.to_string()),
        entities: vec![id.to_string()],
        maker: None,
        model: None,
        members: None,
        hidden: prefs::get()
            .hidden_devices
            .contains(&format!("entity:{id}")),
        grouped: false,
    };
    let (dialog, card) = widgets::dialog(&device.name, 540);
    let about = format!(
        "{} · {}",
        if script { "Script" } else { "Scene" },
        device.area.as_deref().unwrap_or("No room")
    );
    card.append(&widgets::label(&about, "dim"));
    let actions = widgets::hbox(8);
    let run = widgets::labeled_button("media-playback-start-symbolic", "Run");
    run.add_css_class("suggested-action");
    {
        let id = id.to_string();
        run.connect_clicked(move |_| {
            app::call(&id, "turn_on", json!({}));
            app::toast("Done.");
        });
    }
    actions.append(&run);
    // Scripts and scenes made in Home Assistant's editor can be edited there;
    // others (SmartThings, Hue, …) are changed in the app they come from.
    let editor = match (script, e.attributes["id"].as_str()) {
        (true, _) => Some(format!(
            "{}/config/script/edit/{}",
            home.base,
            id.trim_start_matches("script.")
        )),
        (false, Some(scene)) => Some(format!("{}/config/scene/edit/{scene}", home.base)),
        (false, None) => None,
    };
    let mut note = None;
    match editor.filter(|_| !home.base.is_empty()) {
        Some(url) => {
            let open = widgets::labeled_button("network-server-symbolic", "Edit in Home Assistant");
            open.set_tooltip_text(Some("Change what it does, in Home Assistant"));
            let d = dialog.clone();
            open.connect_clicked(move |_| {
                gtk::UriLauncher::new(&url).launch(
                    Some(&d),
                    None::<&gtk::gio::Cancellable>,
                    |_| {},
                );
            });
            actions.append(&open);
        }
        None => {
            let from = match e.platform.as_deref() {
                Some("smartthings") => "SmartThings".to_string(),
                Some("hue") => "Philips Hue".to_string(),
                Some(other) => present::sentence_case(&other.replace('_', " ")),
                None => "the app it comes from".to_string(),
            };
            let label = widgets::label(
                &format!(
                    "Made in {from}: change what it does there. Its name and room can be changed here."
                ),
                "dim",
            );
            label.set_wrap(true);
            note = Some(label);
        }
    }
    card.append(&actions);
    if let Some(note) = note {
        card.append(&note);
    }
    card.append(&settings(
        &device,
        &home,
        if script { "SCRIPT" } else { "SCENE" },
    ));
    let close = gtk::Button::with_label("Close");
    close.set_halign(gtk::Align::End);
    let d = dialog.clone();
    close.connect_clicked(move |_| d.close());
    card.append(&close);
    drop(home);
    dialog.present();
}

/// Hide a device in Nyx Home, or show it again.
pub fn set_hidden(device_id: &str, hidden: bool) {
    prefs::update(|p| {
        p.hidden_devices.retain(|id| id != device_id);
        if hidden {
            p.hidden_devices.push(device_id.to_string());
        }
    });
    app::rebuild();
}

/// Add or remove a device from Overview; true when it was added.
pub fn toggle_favorite(device_id: &str) -> bool {
    let mut added = false;
    prefs::update(|p| {
        if p.favorite_devices.iter().any(|id| id == device_id) {
            p.favorite_devices.retain(|id| id != device_id);
        } else {
            p.favorite_devices.push(device_id.to_string());
            added = true;
        }
    });
    app::rebuild();
    app::toast(if added {
        "Added to Overview."
    } else {
        "Removed from Overview."
    });
    added
}

fn options(e: &Entity) -> Vec<String> {
    e.attributes["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// One entity's row: its name, its value, and a control if it has one.
pub fn row(e: &Entity) -> gtk::Box {
    let control = control(e);
    let value = widgets::label("", "row-value");
    value.set_ellipsize(gtk::pango::EllipsizeMode::End);
    value.set_max_width_chars(22);
    value.set_selectable(true);
    let end = widgets::hbox(8);
    end.append(&value);
    if let Some(c) = &control {
        end.append(c);
    }
    let row = widgets::row(
        &glib::markup_escape_text(&present::short_name(e)),
        "",
        Some(end.upcast_ref()),
    );
    row.set_tooltip_text(Some(&e.entity_id));
    let (id, show_value) = (
        e.entity_id.clone(),
        control.is_none() || matches!(e.domain(), "number" | "input_number" | "sensor"),
    );
    let control = control.clone();
    cards::watch(&row, vec![id.clone()], move |home| {
        let Some(e) = home.entities.get(&id) else {
            return;
        };
        let text = match e.domain() {
            "event" => {
                let kind = e.attributes["event_type"]
                    .as_str()
                    .map(present::sentence_case);
                let when = cards::ago(&e.state);
                [kind, when]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ")
            }
            _ if show_value => present::state_text(e),
            _ => String::new(),
        };
        value.set_text(&text);
        value.set_visible(!text.is_empty());
        if let Some(c) = &control {
            c.set_sensitive(!present::unavailable(e));
        }
    });
    row
}

/// The control an entity's type calls for, kept in step with its state.
pub fn control(e: &Entity) -> Option<gtk::Widget> {
    let id = e.entity_id.clone();
    let widget: gtk::Widget = match e.domain() {
        "switch" | "input_boolean" | "light" | "fan" | "humidifier" => {
            let (sw, set) = cards::switch(&id, &e.name());
            cards::watch(&sw, vec![id.clone()], move |home| {
                if let Some(e) = home.entities.get(&id) {
                    set(e.state == "on");
                }
            });
            sw.upcast()
        }
        "siren" => {
            let id2 = id.clone();
            let on = widgets::two_click("Sound", "Sound it?", move || {
                app::call(&id2, "turn_on", json!({}))
            });
            let id3 = id.clone();
            let off = gtk::Button::with_label("Stop");
            off.connect_clicked(move |_| app::call(&id3, "turn_off", json!({})));
            let b = widgets::hbox(6);
            b.append(&on);
            b.append(&off);
            let (on2, off2) = (on.clone(), off.clone());
            cards::watch(&b, vec![id.clone()], move |home| {
                let sounding = home.entities.get(&id).is_some_and(|e| e.state == "on");
                on2.set_visible(!sounding);
                off2.set_visible(sounding);
            });
            b.upcast()
        }
        "media_player" => {
            let list: Vec<String> = e.attributes["source_list"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if list.is_empty() {
                return None;
            }
            let refs: Vec<&str> = list.iter().map(String::as_str).collect();
            let dd = gtk::DropDown::from_strings(&refs);
            dd.update_property(&[gtk::accessible::Property::Label("Source")]);
            let quiet = Rc::new(Cell::new(false));
            {
                let (id, quiet, list) = (id.clone(), quiet.clone(), list.clone());
                dd.connect_selected_notify(move |d| {
                    if quiet.get() {
                        return;
                    }
                    if let Some(source) = list.get(d.selected() as usize) {
                        app::call(&id, "select_source", json!({"source": source}));
                    }
                });
            }
            let d2 = dd.clone();
            cards::watch(&dd, vec![id.clone()], move |home| {
                let current = home
                    .entities
                    .get(&id)
                    .and_then(|e| e.attributes["source"].as_str());
                quiet.set(true);
                match current.and_then(|s| list.iter().position(|x| x == s)) {
                    Some(i) => d2.set_selected(i as u32),
                    None => d2.set_selected(gtk::INVALID_LIST_POSITION),
                }
                quiet.set(false);
            });
            dd.upcast()
        }
        "select" | "input_select" => {
            let list = options(e);
            if list.is_empty() {
                return None;
            }
            // Raw options like "mopping_after_sweeping" read as words.
            let shown: Vec<String> = list
                .iter()
                .map(|o| {
                    if o.chars().any(char::is_uppercase) {
                        o.clone()
                    } else {
                        present::sentence_case(&o.replace('_', " "))
                    }
                })
                .collect();
            let refs: Vec<&str> = shown.iter().map(String::as_str).collect();
            let dd = gtk::DropDown::from_strings(&refs);
            dd.update_property(&[gtk::accessible::Property::Label(&e.name())]);
            let quiet = Rc::new(Cell::new(false));
            {
                let (id, quiet, list) = (id.clone(), quiet.clone(), list.clone());
                dd.connect_selected_notify(move |d| {
                    if quiet.get() {
                        return;
                    }
                    if let Some(option) = list.get(d.selected() as usize) {
                        app::call(&id, "select_option", json!({"option": option}));
                    }
                });
            }
            let d2 = dd.clone();
            cards::watch(&dd, vec![id.clone()], move |home| {
                let Some(e) = home.entities.get(&id) else {
                    return;
                };
                quiet.set(true);
                match list.iter().position(|o| *o == e.state) {
                    Some(i) => d2.set_selected(i as u32),
                    None => d2.set_selected(gtk::INVALID_LIST_POSITION),
                }
                quiet.set(false);
            });
            dd.upcast()
        }
        "number" | "input_number" => {
            let a = &e.attributes;
            let (min, max) = (
                a["min"].as_f64().unwrap_or(0.0),
                a["max"].as_f64().unwrap_or(100.0),
            );
            let step = a["step"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
            let spin = gtk::SpinButton::with_range(min, max.max(min + step), step);
            spin.update_property(&[gtk::accessible::Property::Label(&e.name())]);
            let quiet = Rc::new(Cell::new(false));
            {
                let (id, quiet, debounce) = (id.clone(), quiet.clone(), Debounce::default());
                spin.connect_value_changed(move |s| {
                    if quiet.get() {
                        return;
                    }
                    let (id, v) = (id.clone(), s.value());
                    debounce.run(600, move || {
                        app::call(&id, "set_value", json!({"value": v}))
                    });
                });
            }
            let s2 = spin.clone();
            cards::watch(&spin, vec![id.clone()], move |home| {
                if let Some(v) = home
                    .entities
                    .get(&id)
                    .and_then(|e| e.state.parse::<f64>().ok())
                {
                    quiet.set(true);
                    s2.set_value(v);
                    quiet.set(false);
                }
            });
            spin.upcast()
        }
        "button" | "input_button" | "scene" | "script" => {
            let (label, service) = if matches!(e.domain(), "scene" | "script") {
                ("Run", "turn_on")
            } else {
                ("Press", "press")
            };
            let b = gtk::Button::with_label(label);
            b.update_property(&[gtk::accessible::Property::Label(&format!(
                "{label} {}",
                e.name()
            ))]);
            b.connect_clicked(move |_| app::call(&id, service, json!({})));
            b.upcast()
        }
        "lock" => {
            let id2 = id.clone();
            let unlock = widgets::two_click("Unlock", "Unlock?", move || {
                app::call(&id2, "unlock", json!({}))
            });
            let id3 = id.clone();
            let lock = gtk::Button::with_label("Lock");
            lock.connect_clicked(move |_| app::call(&id3, "lock", json!({})));
            let b = widgets::hbox(6);
            b.append(&unlock);
            b.append(&lock);
            let (u2, l2) = (unlock.clone(), lock.clone());
            cards::watch(&b, vec![id.clone()], move |home| {
                let locked = home.entities.get(&id).is_some_and(|e| e.state == "locked");
                u2.set_visible(locked);
                l2.set_visible(!locked);
            });
            b.upcast()
        }
        "cover" => {
            let b = widgets::hbox(4);
            for (label, service, icon) in present::cover_actions(e) {
                let button = widgets::icon_button(icon, label);
                let id = id.clone();
                button.connect_clicked(move |_| app::call(&id, service, json!({})));
                b.append(&button);
            }
            b.upcast()
        }
        "camera" | "image" => {
            let thumb = viewer::thumbnail(&id, &e.name(), 72);
            thumb.set_size_request(120, -1);
            thumb.upcast()
        }
        _ => return None,
    };
    Some(widget)
}
