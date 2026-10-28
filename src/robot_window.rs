//! A robot's own window, like the vendor apps: the live map with Home's layer
//! (tap a room or zone), and a side panel with its status, commands, the
//! settings that matter and its station or mower actions. The floor plan
//! refreshes while the robot works.
use crate::{
    app, cards, details,
    device::{self, Device},
    ha::Entity,
    home_features, present,
    viewer::{self, MapView},
};
use gtk::{glib, prelude::*};
use nyx_ui::widgets;
use std::{collections::BTreeMap, rc::Rc};

/// How often the floor plan refreshes while the robot works.
const REFRESH_SECONDS: u32 = 10;

/// `domain:suffix` of the settings shown in the panel, in order.
const VACUUM_SETTINGS: &[&str] = &[
    "select:_cleaning_mode",
    "select:_suction_level",
    "select:_cleaning_route",
    "select:_carpet_cleaning",
    "select:_cleangenius",
];
const MOWER_SETTINGS: &[&str] = &[
    "select:_mowing_action",
    "number:_selected_map_mowing_height",
    "select:_selected_mowing_efficiency",
    "select:_selected_mowing_direction_mode",
    "switch:_rain_protection",
    "switch:_real_time_location",
];
/// Buttons, and whether each needs a second click (it can't be undone).
const VACUUM_ACTIONS: &[(&str, bool)] = &[
    ("button:_start_auto_empty", false),
    ("button:_self_clean", false),
    ("button:_manual_drying", false),
    ("button:_base_station_cleaning", false),
];
const MOWER_ACTIONS: &[(&str, bool)] = &[
    ("button:_find_my_mower", false),
    ("button:_dock_without_ending_session", false),
    ("button:_end_current_task", true),
];

/// The entities a robot's panel shows.
#[derive(Debug, Default, PartialEq)]
pub struct Controls {
    pub settings: Vec<String>,
    pub actions: Vec<(String, bool)>,
}

fn matching<'a>(
    device: &'a Device,
    entities: &'a BTreeMap<String, Entity>,
    spec: &str,
) -> Option<&'a Entity> {
    let (domain, suffix) = spec.split_once(':')?;
    device
        .entities
        .iter()
        .filter_map(|id| entities.get(id))
        .find(|e| e.domain() == domain && e.entity_id.ends_with(suffix) && !present::unavailable(e))
}

/// Pick a robot's panel controls by entity name, leaving out what it lacks.
pub fn robot_controls(device: &Device, entities: &BTreeMap<String, Entity>) -> Controls {
    let mower = device
        .primary(entities)
        .is_some_and(|e| e.domain() == "lawn_mower");
    let (settings, actions) = if mower {
        (MOWER_SETTINGS, MOWER_ACTIONS)
    } else {
        (VACUUM_SETTINGS, VACUUM_ACTIONS)
    };
    Controls {
        settings: settings
            .iter()
            .filter_map(|s| matching(device, entities, s))
            .map(|e| e.entity_id.clone())
            .collect(),
        actions: actions
            .iter()
            .filter_map(|(s, confirm)| {
                matching(device, entities, s).map(|e| (e.entity_id.clone(), *confirm))
            })
            .collect(),
    }
}

/// Whether the robot is out working, so its map is changing.
pub fn working(e: &Entity) -> bool {
    matches!(
        e.state.as_str(),
        "cleaning" | "returning" | "mowing" | "edge_mowing" | "spot_cleaning"
    )
}

/// This session's progress from a vacuum's attributes: area, time, percent.
fn progress(e: &Entity) -> Vec<String> {
    let a = &e.attributes;
    let mut out = Vec::new();
    if let Some(area) = a["cleaned_area"].as_f64().filter(|v| *v > 0.0) {
        out.push(format!("{area} m²"));
    }
    if let Some(min) = a["cleaning_time"].as_f64().filter(|v| *v > 0.0) {
        out.push(format!("{min} min"));
    }
    if let Some(p) = a["cleaning_progress"].as_f64().filter(|v| *v > 0.0) {
        out.push(format!("{p}% done"));
    }
    out
}

fn section(panel: &gtk::Box, title: &str) -> gtk::Box {
    panel.append(&widgets::label(title, "group-title"));
    let list = widgets::vbox(0);
    list.add_css_class("group-list");
    panel.append(&list);
    list
}

pub fn open(device_id: &str) {
    let Some(app) = app::get() else { return };
    let (device, robot, map, controls) = {
        let home = app.home.borrow();
        let Some(device) = home
            .device_list(true)
            .into_iter()
            .find(|d| d.id == device_id)
        else {
            app::toast("That robot is no longer in Home Assistant.");
            return;
        };
        let Some(robot) = device.primary(&home.entities).cloned() else {
            return;
        };
        let members: Vec<&Entity> = device
            .entities
            .iter()
            .filter_map(|id| home.entities.get(id))
            .collect();
        let map = present::robot_map(&robot, members.iter().copied()).map(|m| m.entity_id.clone());
        let controls = robot_controls(&device, &home.entities);
        (device, robot, map, controls)
    };
    let mower = robot.domain() == "lawn_mower";

    let window = gtk::Window::builder()
        .title(&device.name)
        .transient_for(&app.window)
        .default_width((app.window.width() as f64 * 0.9).max(720.0) as i32)
        .default_height((app.window.height() as f64 * 0.9).max(520.0) as i32)
        .build();
    window.set_application(app.window.application().as_ref());
    window.add_css_class("home-window");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    let header = nyx_ui::chrome::app_header(&window, crate::icons::MAP, &device.name);
    header.end.set_spacing(4);

    // The map.
    let view = MapView::new(&format!("{} map", device.name));
    match &map {
        Some(camera) => {
            viewer::zoom_buttons(&view, &header.end);
            let (v, note) = (view.clone(), view.note.clone());
            viewer::load(camera, move |result| match result {
                Ok(texture) => v.show_texture(&texture),
                Err(error) => note.set_text(&error),
            });
            view.keys(&window);
        }
        None => view
            .note
            .set_text("This robot has no map in Home Assistant."),
    }
    let close = gtk::Button::with_label("Close");
    close.set_tooltip_text(Some("Close (Esc)"));
    header.end.append(&close);

    // The panel.
    let panel = widgets::vbox(10);
    panel.add_css_class("robot-panel");
    let state = widgets::label("", "robot-state");
    let detail = widgets::label("", "dim");
    detail.set_wrap(true);
    panel.append(&state);
    panel.append(&detail);

    let commands = widgets::hbox(6);
    commands.add_css_class("robot-actions");
    for action in home_features::robot_actions(&robot) {
        let label = action.label(robot.domain());
        let b = widgets::icon_button(action.icon(), label);
        b.remove_css_class("flat");
        b.add_css_class("robot-action");
        b.update_property(&[gtk::accessible::Property::Label(label)]);
        let id = robot.entity_id.clone();
        b.connect_clicked(move |_| {
            let entity = app::get().and_then(|a| a.home.borrow().entities.get(&id).cloned());
            if let Some(entity) = entity {
                cards::run_robot(&entity, action);
            }
        });
        commands.append(&b);
    }
    panel.append(&commands);
    if map.is_some() {
        let hint = widgets::label(
            if mower {
                "Tap a zone on the map to mow it."
            } else {
                "Tap a room on the map to clean it."
            },
            "dim",
        );
        hint.set_wrap(true);
        panel.append(&hint);
    }

    let mut watched = vec![robot.entity_id.clone()];
    {
        let home = app.home.borrow();
        if !controls.settings.is_empty() {
            let list = section(&panel, if mower { "MOWING" } else { "CLEANING" });
            for id in &controls.settings {
                let Some(e) = home.entities.get(id) else {
                    continue;
                };
                let row = if id.ends_with("_real_time_location") {
                    widgets::row(
                        "Live position",
                        "Report where the mower is while it mows",
                        details::control(e).as_ref(),
                    )
                } else {
                    let name = present::short_name(e);
                    let title = name.strip_prefix("Selected ").unwrap_or(&name);
                    widgets::row(
                        &present::sentence_case(title),
                        "",
                        details::control(e).as_ref(),
                    )
                };
                list.append(&row);
                watched.push(id.clone());
            }
        }
        if !controls.actions.is_empty() {
            panel.append(&widgets::label(
                if mower { "MOWER" } else { "STATION" },
                "group-title",
            ));
            let list = widgets::vbox(6);
            panel.append(&list);
            for (id, confirm) in &controls.actions {
                let Some(e) = home.entities.get(id) else {
                    continue;
                };
                let name = present::short_name(e);
                let press = {
                    let id = id.clone();
                    move || app::call(&id, "press", serde_json::json!({}))
                };
                let button = if *confirm {
                    widgets::two_click(&name, &format!("{name}?"), press)
                } else {
                    let b = gtk::Button::with_label(&name);
                    b.connect_clicked(move |_| press());
                    b
                };
                button.set_hexpand(true);
                list.append(&button);
            }
        }
    }
    let all = widgets::labeled_button("view-more-symbolic", "All settings…");
    all.set_halign(gtk::Align::Start);
    {
        let id = device.id.clone();
        all.connect_clicked(move |_| details::open(&id));
    }
    panel.append(&all);
    let side = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&panel)
        .min_content_width(420)
        .build();
    side.add_css_class("robot-side");

    // The map beside the panel; drag the divider to give either more room.
    let body = gtk::Paned::new(gtk::Orientation::Horizontal);
    body.set_start_child(Some(&view.root));
    body.set_end_child(Some(&side));
    body.set_resize_start_child(true);
    body.set_resize_end_child(false);
    body.set_shrink_end_child(false);
    body.set_wide_handle(true);
    body.set_vexpand(true);
    let frame = widgets::vbox(0);
    frame.add_css_class("window-frame");
    frame.append(&header.root);
    frame.append(&body);
    window.set_child(Some(&frame));
    // Home's layer once the map is in the window, so its watcher stays.
    if let Some(camera) = &map {
        view.attach_layer(camera);
    }

    // Status, kept current.
    let battery = device
        .find(&app.home.borrow().entities, |e| {
            e.domain() == "sensor" && device::device_class(e) == "battery"
        })
        .map(|e| e.entity_id.clone());
    if let Some(b) = &battery {
        watched.push(b.clone());
    }
    let id = robot.entity_id.clone();
    cards::watch(&panel, watched.clone(), move |home| {
        let Some(r) = home.entities.get(&id) else {
            return;
        };
        state.set_text(&present::state_text(r));
        let mut parts = Vec::new();
        let level = battery
            .as_ref()
            .and_then(|b| home.entities.get(b))
            .and_then(|e| e.state.parse::<f64>().ok())
            .or_else(|| r.attributes["battery_level"].as_f64());
        if let Some(l) = level {
            parts.push(format!("{}% battery", l.round()));
        }
        if working(r) {
            parts.extend(progress(r));
        }
        detail.set_text(&parts.join(" · "));
    });
    if let Some(app) = app::get() {
        cards::notify(&app.home.borrow(), Some(&watched.into_iter().collect()));
    }

    // While the robot works, a fresh floor plan every few seconds.
    if let Some(camera) = map {
        let (w, v, id) = (
            window.downgrade(),
            Rc::downgrade(&view),
            robot.entity_id.clone(),
        );
        glib::timeout_add_seconds_local(REFRESH_SECONDS, move || {
            let (Some(_), Some(v)) = (w.upgrade(), v.upgrade()) else {
                return glib::ControlFlow::Break;
            };
            let busy = app::get()
                .and_then(|a| a.home.borrow().entities.get(&id).map(working))
                .unwrap_or(false);
            if busy {
                viewer::load(&camera, move |result| {
                    if let Ok(texture) = result {
                        v.show_texture(&texture);
                    }
                });
            }
            glib::ControlFlow::Continue
        });
    }

    let w = window.clone();
    close.connect_clicked(move |_| w.close());
    let keys = gtk::EventControllerKey::new();
    let w = window.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            w.close();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    window.add_controller(keys);
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entity(id: &str, state: &str) -> Entity {
        let mut e: Entity =
            serde_json::from_value(json!({"entity_id": id, "state": state, "attributes": {}}))
                .unwrap();
        e.device_id = Some("robot".into());
        e
    }

    fn device_of(list: &[Entity]) -> (Device, BTreeMap<String, Entity>) {
        let entities: BTreeMap<String, Entity> = list
            .iter()
            .map(|e| (e.entity_id.clone(), e.clone()))
            .collect();
        let device = device::group(&entities, &Default::default(), true)
            .into_iter()
            .next()
            .unwrap();
        (device, entities)
    }

    #[test]
    fn vacuum_panel_picks_its_settings_and_station() {
        let (device, entities) = device_of(&[
            entity("vacuum.z60", "docked"),
            entity("select.z60_suction_level", "standard"),
            entity("select.z60_cleaning_mode", "sweeping"),
            entity("select.z60_mop_pad_humidity", "unavailable"),
            entity("select.z60_cleangenius", "unavailable"),
            entity("button.z60_start_auto_empty", "unknown"),
            entity("button.z60_reset_filter", "unknown"),
        ]);
        let c = robot_controls(&device, &entities);
        assert_eq!(
            c.settings,
            ["select.z60_cleaning_mode", "select.z60_suction_level"]
        );
        assert_eq!(
            c.actions,
            [("button.z60_start_auto_empty".to_string(), false)]
        );
    }

    #[test]
    fn mower_panel_asks_before_ending_a_task() {
        let (device, entities) = device_of(&[
            entity("lawn_mower.lidax", "docked"),
            entity("number.lidax_selected_map_mowing_height", "4.0"),
            entity("switch.lidax_real_time_location", "off"),
            entity("button.lidax_end_current_task", "unknown"),
            entity("button.lidax_find_my_mower", "unknown"),
        ]);
        let c = robot_controls(&device, &entities);
        assert_eq!(
            c.settings,
            [
                "number.lidax_selected_map_mowing_height",
                "switch.lidax_real_time_location"
            ]
        );
        assert_eq!(
            c.actions,
            [
                ("button.lidax_find_my_mower".to_string(), false),
                ("button.lidax_end_current_task".to_string(), true)
            ]
        );
        assert!(working(&entity("lawn_mower.lidax", "mowing")));
        assert!(!working(&entity("lawn_mower.lidax", "docked")));
    }
}
