//! Light groups made in Home Assistant: one light that stands for several,
//! kept in step by Home Assistant (and usable in its own app, automations and
//! voice assistants). Made, changed and deleted through its config flows.
use crate::{app, connection, device::Device, ha};
use gtk::prelude::*;
use nyx_ui::widgets;
use serde_json::json;
use std::{rc::Rc, time::Duration};

/// Single lights (not groups) Home knows about: `(entity, name, room)`.
fn lights(devices: &[Device]) -> Vec<(String, String, Option<String>)> {
    devices
        .iter()
        .filter_map(|d| Some((d.primary.clone()?, d.name.clone(), d.area.clone())))
        .collect()
}

/// A checklist of lights; `checked` tells which start ticked.
fn checklist(
    all: &[(String, String, Option<String>)],
    checked: impl Fn(&str, Option<&str>) -> bool,
) -> (gtk::ScrolledWindow, Rc<Vec<(String, gtk::CheckButton)>>) {
    let list = widgets::vbox(0);
    list.add_css_class("group-list");
    let mut checks = Vec::new();
    for (id, name, room) in all {
        let check = gtk::CheckButton::new();
        check.set_active(checked(id, room.as_deref()));
        let text = match room {
            Some(room) => format!("{name} · {room}"),
            None => name.clone(),
        };
        check.set_label(Some(&text));
        check.add_css_class("light-check");
        list.append(&check);
        checks.push((id.clone(), check));
    }
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(160)
        .max_content_height(320)
        .propagate_natural_height(true)
        .child(&list)
        .build();
    (scroll, Rc::new(checks))
}

fn chosen(checks: &[(String, gtk::CheckButton)]) -> Vec<String> {
    checks
        .iter()
        .filter(|(_, c)| c.is_active())
        .map(|(id, _)| id.clone())
        .collect()
}

fn field(card: &gtk::Box, title: &str, widget: &impl IsA<gtk::Widget>) {
    card.append(&widgets::label(title, "field-label"));
    card.append(widget);
}

/// Save a room's lights as a light group.
pub fn create_for_room(room: &str) {
    let Some(app) = app::get() else { return };
    let lights = app
        .home
        .borrow()
        .room_light_sets()
        .into_iter()
        .find(|(r, _)| r == room)
        .map(|(_, l)| l)
        .unwrap_or_default();
    create(Some(room.to_string()), lights);
}

/// Make a new light group: a name, a room, its lights, and whether to hide
/// the lights it holds.
pub fn create(room: Option<String>, preset: Vec<String>) {
    let Some(app) = app::get() else { return };
    let (devices, areas) = {
        let home = app.home.borrow();
        (home.lights(), home.areas.clone())
    };
    let all = lights(&devices);
    if all.len() < 2 {
        app::toast("A light group needs at least two lights.");
        return;
    }
    let (dialog, card) = widgets::dialog("New light group", 460);
    let note = widgets::label(
        "Home Assistant keeps the lights in a group in step: one switch, one brightness and one color for all of them.",
        "dim",
    );
    note.set_wrap(true);
    card.append(&note);

    let name = gtk::Entry::new();
    name.set_text(
        &room
            .as_ref()
            .map(|r| format!("{r} lights"))
            .unwrap_or_default(),
    );
    name.set_placeholder_text(Some("Bedroom lamps"));
    field(&card, "Name", &name);

    let mut rooms: Vec<String> = vec!["No room".into()];
    rooms.extend(areas.iter().map(|a| a.name.clone()));
    let refs: Vec<&str> = rooms.iter().map(String::as_str).collect();
    let picker = gtk::DropDown::from_strings(&refs);
    let at = room
        .as_ref()
        .and_then(|r| rooms.iter().position(|x| x == r))
        .unwrap_or(0);
    picker.set_selected(at as u32);
    field(&card, "Room", &picker);

    let (scroll, checks) = checklist(&all, |id, light_room| {
        if preset.is_empty() {
            room.is_some() && light_room == room.as_deref()
        } else {
            preset.iter().any(|p| p == id)
        }
    });
    field(&card, "Lights", &scroll);

    let hide_row = widgets::hbox(10);
    let hide = gtk::Switch::new();
    hide.set_valign(gtk::Align::Center);
    hide.update_property(&[gtk::accessible::Property::Label("Hide the grouped lights")]);
    let hide_text = widgets::label("Hide the grouped lights, so only the group shows", "");
    hide_text.set_wrap(true);
    hide_text.set_hexpand(true);
    hide_row.append(&hide_text);
    hide_row.append(&hide);
    card.append(&hide_row);

    let buttons = widgets::hbox(8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let make = gtk::Button::with_label("Make group");
    make.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&make);
    card.append(&buttons);
    let ready = {
        let (name, checks, make) = (name.clone(), checks.clone(), make.clone());
        Rc::new(move || {
            make.set_sensitive(!name.text().trim().is_empty() && chosen(&checks).len() >= 2);
        })
    };
    ready();
    {
        let ready = ready.clone();
        name.connect_changed(move |_| ready());
    }
    for (_, check) in checks.iter() {
        let ready = ready.clone();
        check.connect_toggled(move |_| ready());
    }
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let d = dialog.clone();
    make.connect_clicked(move |b| {
        let title = name.text().trim().to_string();
        let members = chosen(&checks);
        let hide_members = hide.is_active();
        let area = picker
            .selected()
            .checked_sub(1)
            .and_then(|i| areas.get(i as usize))
            .map(|a| a.id.clone());
        b.set_sensitive(false);
        let (d, b) = (d.clone(), b.clone());
        app::ha_try(
            move |base, token| async move {
                let entry =
                    ha::create_light_group(&base, &token, &title, &members, hide_members).await?;
                if let Some(area) = area {
                    // The group's entity appears a moment after the entry.
                    for _ in 0..10 {
                        if let Some(entity) = ha::entry_entity(&base, &token, &entry).await? {
                            ha::ws_command(
                                &base,
                                &token,
                                json!({"type": "config/entity_registry/update",
                                       "entity_id": entity, "area_id": area}),
                            )
                            .await?;
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                }
                Ok(())
            },
            move |result| match result {
                Ok(()) => {
                    d.close();
                    app::toast("Light group made.");
                    connection::reconnect();
                }
                Err(error) => {
                    // Leave the dialog open to try again.
                    app::toast(&error);
                    b.set_sensitive(true);
                }
            },
        );
    });
    dialog.present();
}

/// Change which lights a group holds.
pub fn edit(device_id: &str) {
    let Some(app) = app::get() else { return };
    let (group, entry, hidden, all) = {
        let home = app.home.borrow();
        let Some(group) = home
            .device_list(true)
            .into_iter()
            .find(|d| d.id == device_id)
        else {
            return;
        };
        let Some(entry) = group.group_entry(&home.entities) else {
            app::toast(
                "This group wasn't made in Home Assistant's UI, so it can't be changed here.",
            );
            return;
        };
        let members = group.members.clone().unwrap_or_default();
        let hidden = members
            .iter()
            .any(|id| home.entities.get(id).is_some_and(|e| e.hidden));
        (group, entry, hidden, lights(&home.lights()))
    };
    let members = group.members.clone().unwrap_or_default();
    let (dialog, card) = widgets::dialog(&format!("Lights in {}", group.name), 460);
    let (scroll, checks) = checklist(&all, |id, _| members.iter().any(|m| m == id));
    card.append(&scroll);
    let hide_row = widgets::hbox(10);
    let hide = gtk::Switch::new();
    hide.set_active(hidden);
    hide.set_valign(gtk::Align::Center);
    hide.update_property(&[gtk::accessible::Property::Label("Hide the grouped lights")]);
    let hide_text = widgets::label("Hide the grouped lights, so only the group shows", "");
    hide_text.set_wrap(true);
    hide_text.set_hexpand(true);
    hide_row.append(&hide_text);
    hide_row.append(&hide);
    card.append(&hide_row);
    let buttons = widgets::hbox(8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&save);
    card.append(&buttons);
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    let d = dialog.clone();
    save.connect_clicked(move |_| {
        let members = chosen(&checks);
        if members.len() < 2 {
            app::toast("Choose at least two lights.");
            return;
        }
        let (entry, hide_members, d) = (entry.clone(), hide.is_active(), d.clone());
        app::ha_task(
            move |base, token| async move {
                ha::edit_light_group(&base, &token, &entry, &members, hide_members).await
            },
            move |()| {
                d.close();
                app::toast("Light group saved.");
                connection::reconnect();
            },
        );
    });
    dialog.present();
}

/// Delete a light group from Home Assistant; its lights stay.
pub fn delete(entry: &str, name: &str) {
    let (entry, name) = (entry.to_string(), name.to_string());
    app::ha_task(
        move |base, token| async move { ha::delete_config_entry(&base, &token, &entry).await },
        move |()| {
            app::toast(&format!("Deleted {name}."));
            connection::reconnect();
        },
    );
}
