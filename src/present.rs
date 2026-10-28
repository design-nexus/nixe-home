//! How Home Assistant entities and automations read on screen: friendly
//! states, icons, the control each device gets, and plain descriptions.
//! Kept free of GTK so it can be tested.
use crate::ha::Entity;
use nyx_home::automation::{NodeKind, Owner, Workflow};

pub fn unavailable(e: &Entity) -> bool {
    e.state == "unavailable"
}

fn device_class(e: &Entity) -> &str {
    e.attributes["device_class"].as_str().unwrap_or("")
}

fn features(e: &Entity) -> u64 {
    e.attributes["supported_features"].as_u64().unwrap_or(0)
}

/// A tile's name: the entity's name without its device's name in front
/// ("Air Purifier Air quality" on the Air Purifier reads "Air quality").
pub fn short_name(e: &Entity) -> String {
    let name = e.name().split_whitespace().collect::<Vec<_>>().join(" ");
    let Some(device) = e.device_name.as_deref() else {
        return name;
    };
    let device = device.split_whitespace().collect::<Vec<_>>().join(" ");
    match name.strip_prefix(&device) {
        // "Z60 Ultra Z60 Ultra": the device name twice is just the device.
        Some(rest) if rest.trim() == device => device,
        Some(rest) if !rest.trim().is_empty() && rest.starts_with(' ') => sentence(rest.trim()),
        _ => name,
    }
}

pub fn sentence_case(text: &str) -> String {
    sentence(text)
}

fn sentence(text: &str) -> String {
    let text = text.replace('_', " ");
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// A number as Home Assistant would show it: whole, or with one decimal.
fn number(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.1}")
    }
}

pub fn state_text(e: &Entity) -> String {
    match e.state.as_str() {
        "unavailable" => return "Unavailable".into(),
        "unknown" => return "Unknown".into(),
        _ => {}
    }
    let on = e.state == "on";
    match e.domain() {
        "binary_sensor" => match device_class(e) {
            "door" | "garage_door" | "opening" | "window" => {
                if on {
                    "Open"
                } else {
                    "Closed"
                }
            }
            "motion" | "occupancy" | "presence" | "smoke" | "gas" | "carbon_monoxide" | "sound"
            | "vibration" | "tamper" | "problem" | "safety" => {
                if on {
                    "Detected"
                } else {
                    "Clear"
                }
            }
            "moisture" => {
                if on {
                    "Wet"
                } else {
                    "Dry"
                }
            }
            "connectivity" => {
                if on {
                    "Connected"
                } else {
                    "Disconnected"
                }
            }
            "battery" => {
                if on {
                    "Low"
                } else {
                    "Normal"
                }
            }
            "lock" => {
                if on {
                    "Unlocked"
                } else {
                    "Locked"
                }
            }
            "plug" | "power" => {
                if on {
                    "Plugged in"
                } else {
                    "Unplugged"
                }
            }
            _ => {
                if on {
                    "On"
                } else {
                    "Off"
                }
            }
        }
        .into(),
        "light" if on => match brightness(e) {
            Some(pct) if pct > 0.0 => format!("On · {}%", pct.round() as i64),
            _ => "On".into(),
        },
        "person" | "device_tracker" => match e.state.as_str() {
            "home" => "Home".into(),
            "not_home" => "Away".into(),
            other => sentence(other),
        },
        "climate" => {
            let mode = sentence(&e.state);
            match e.attributes["current_temperature"].as_f64() {
                Some(t) => format!("{}° · {mode}", number(t)),
                None => mode,
            }
        }
        "media_player" => {
            let title = e.attributes["media_title"].as_str().unwrap_or("");
            if title.is_empty() {
                sentence(&e.state)
            } else {
                format!("{} · {title}", sentence(&e.state))
            }
        }
        "update" => if on { "Update available" } else { "Up to date" }.into(),
        "scene" | "button" | "input_button" => "Ready".into(),
        "sensor" | "number" | "input_number" => {
            let unit = e.attributes["unit_of_measurement"].as_str().unwrap_or("");
            let value = e
                .state
                .parse::<f64>()
                .map(number)
                .unwrap_or_else(|_| sentence(&e.state));
            match unit {
                "" => value,
                "%" | "°C" | "°F" => format!("{value}{unit}"),
                _ => format!("{value} {unit}"),
            }
        }
        _ => sentence(&e.state),
    }
}

pub fn toggle_service(on: bool) -> &'static str {
    if on { "turn_on" } else { "turn_off" }
}

/// A light's brightness in percent, when the light can dim.
pub fn brightness(e: &Entity) -> Option<f64> {
    if e.domain() != "light" {
        return None;
    }
    let modes = e.attributes["supported_color_modes"].as_array()?;
    if modes.iter().all(|m| m.as_str() == Some("onoff")) {
        return None;
    }
    Some(
        e.attributes["brightness"]
            .as_f64()
            .map_or(0.0, |b| (b / 255.0 * 100.0).clamp(0.0, 100.0)),
    )
}

/// Cover buttons the entity supports: (label, service, icon).
pub fn cover_actions(e: &Entity) -> Vec<(&'static str, &'static str, &'static str)> {
    if e.domain() != "cover" || unavailable(e) {
        return Vec::new();
    }
    let f = features(e);
    [
        (1, "Open", "open_cover", "go-up-symbolic"),
        (8, "Stop", "stop_cover", "media-playback-stop-symbolic"),
        (2, "Close", "close_cover", "go-down-symbolic"),
    ]
    .into_iter()
    .filter(|(bit, ..)| f & bit != 0)
    .map(|(_, label, service, icon)| (label, service, icon))
    .collect()
}

/// The map to show on a robot's tile: a map camera or image on the robot's
/// device, the current map before saved or diagnostic ones.
pub fn robot_map<'a>(
    robot: &Entity,
    entities: impl IntoIterator<Item = &'a Entity>,
) -> Option<&'a Entity> {
    let device = robot.device_id.as_ref()?;
    entities
        .into_iter()
        .filter(|e| {
            let name = e.name().to_lowercase();
            matches!(e.domain(), "camera" | "image")
                && e.device_id.as_ref() == Some(device)
                && name.contains("map")
                && !e.hidden
                && !unavailable(e)
                && ![
                    "saved",
                    "all maps",
                    "diagnostic",
                    "live path",
                    "wifi",
                    "data",
                ]
                .iter()
                .any(|other| name.contains(other))
        })
        .min_by_key(|e| {
            (
                !e.name().to_lowercase().contains("current"),
                e.entity_id.clone(),
            )
        })
}

/// Something about a device that needs attention.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub title: String,
    pub place: String,
    pub problem: String,
}

/// Device health: low batteries and updates, and unavailable devices, with a
/// device's unavailable entities counted once. Unavailable diagnostic and
/// hidden entities are left out; they rarely mean the device is broken.
pub fn health<'a>(entities: impl IntoIterator<Item = &'a Entity>) -> Vec<Issue> {
    let mut issues = Vec::new();
    let mut unavailable: std::collections::BTreeMap<String, (String, usize)> = Default::default();
    for e in entities {
        let Some(problem) = crate::home_features::health_issue(e) else {
            continue;
        };
        if problem == "Unavailable" {
            if e.entity_category.is_some() || e.hidden {
                continue;
            }
            let key = e.device_id.clone().unwrap_or_else(|| e.entity_id.clone());
            let entry = unavailable
                .entry(key)
                .or_insert_with(|| (e.device_name.clone().unwrap_or_else(|| e.name()), 0));
            entry.1 += 1;
            continue;
        }
        issues.push(Issue {
            title: e.name(),
            place: place(e),
            problem,
        });
    }
    let mut offline: Vec<Issue> = unavailable
        .into_values()
        .map(|(title, count)| Issue {
            title,
            place: String::new(),
            problem: if count == 1 {
                "Unavailable".into()
            } else {
                format!("{count} entities unavailable")
            },
        })
        .collect();
    offline.sort_by_key(|i| i.title.to_lowercase());
    offline.extend(issues);
    offline
}

fn place(e: &Entity) -> String {
    match (&e.area_name, &e.device_name) {
        (Some(area), Some(device)) if *device != e.name() => format!("{device} · {area}"),
        (Some(area), _) => area.clone(),
        (None, Some(device)) => device.clone(),
        (None, None) => String::new(),
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Summary {
    pub lights_on: usize,
    pub lights: usize,
    pub open: usize,
    pub people_home: usize,
    pub people: usize,
    pub climate: Option<String>,
    pub issues: usize,
}

pub fn summarize<'a>(entities: impl IntoIterator<Item = &'a Entity> + Clone) -> Summary {
    let mut s = Summary {
        issues: health(entities.clone()).len(),
        ..Summary::default()
    };
    for e in entities {
        match e.domain() {
            "light" if e.entity_category.is_none() && !e.hidden => {
                s.lights += 1;
                s.lights_on += usize::from(e.state == "on");
            }
            "binary_sensor"
                if matches!(
                    device_class(e),
                    "door" | "garage_door" | "opening" | "window"
                ) && e.state == "on" =>
            {
                s.open += 1
            }
            "cover" if e.state == "open" => s.open += 1,
            "person" => {
                s.people += 1;
                s.people_home += usize::from(e.state == "home");
            }
            "climate" if s.climate.is_none() => {
                s.climate = e.attributes["current_temperature"]
                    .as_f64()
                    .map(|t| format!("{}°", number(t)))
            }
            _ => {}
        }
    }
    s
}

pub fn relative_time(then: i64, now: i64) -> String {
    let ago = (now - then).max(0);
    match ago {
        0..45 => "Just now".into(),
        45..90 => "1 min ago".into(),
        90..3600 => format!("{} min ago", (ago + 30) / 60),
        3600..7200 => "1 hour ago".into(),
        7200..86_400 => format!("{} hours ago", ago / 3600),
        86_400..172_800 => "Yesterday".into(),
        _ => format!("{} days ago", ago / 86_400),
    }
}

pub fn duration(seconds: i64) -> String {
    match seconds.max(0) {
        0 => "under 1 s".into(),
        s @ 1..60 => format!("{s} s"),
        s @ 60..3600 => format!("{} min {} s", s / 60, s % 60),
        s => format!("{} h {} min", s / 3600, (s % 3600) / 60),
    }
}

fn quote(text: &str) -> String {
    let short: String = text.chars().take(40).collect();
    if short.len() < text.len() {
        format!("“{short}…”")
    } else {
        format!("“{short}”")
    }
}

/// A node in a sentence, e.g. "When Firefox opens" or "Turn on Office lights".
pub fn node_label(kind: &NodeKind, names: &dyn Fn(&str) -> String) -> String {
    match kind {
        NodeKind::Manual => "Run manually".into(),
        NodeKind::Startup => "When the desktop starts".into(),
        NodeKind::Interval { seconds } => match seconds / 60 {
            1 => "Every minute".into(),
            m if m % 60 == 0 => format!("Every {} hours", m / 60),
            m => format!("Every {m} minutes"),
        },
        NodeKind::WindowOpened { class } => format!("When {class} opens"),
        NodeKind::WindowClosed { class } => format!("When {class} closes"),
        NodeKind::EntityState { entity_id, state } => {
            format!("When {} becomes {state}", names(entity_id))
        }
        NodeKind::AppRunning { class } => format!("If {class} is running"),
        NodeKind::EntityIs { entity_id, state } => format!("If {} is {state}", names(entity_id)),
        NodeKind::BranchAppRunning { class } => format!("Is {class} running?"),
        NodeKind::VariableEquals { name, value } => format!("If {name} is {}", quote(value)),
        NodeKind::Notify { title, .. } => format!("Notify {}", quote(title)),
        NodeKind::SwitchWorkspace { number } => format!("Go to workspace {number}"),
        NodeKind::SetVolume { percent } => format!("Set volume to {percent}%"),
        NodeKind::LaunchApplication { desktop_id } => {
            format!("Open {}", desktop_id.trim_end_matches(".desktop"))
        }
        NodeKind::SetEntity { entity_id, turn_on } => format!(
            "Turn {} {}",
            if *turn_on { "on" } else { "off" },
            names(entity_id)
        ),
        NodeKind::ActivateScene { entity_id } => format!("Activate {}", names(entity_id)),
        NodeKind::Parallel => "Run paths at once".into(),
        NodeKind::Delay { seconds } => format!("Wait {}", duration(*seconds as i64)),
        NodeKind::Repeat { times } => format!("Repeat {times} times"),
        NodeKind::WaitWindowOpened { class, .. } => format!("Wait for {class} to open"),
        NodeKind::WaitEntityState {
            entity_id, state, ..
        } => {
            format!("Wait for {} to be {state}", names(entity_id))
        }
        NodeKind::SetVariable { name, value } => format!("Set {name} to {}", quote(value)),
        NodeKind::Stop => "Stop".into(),
    }
}

/// One line for an automation list: its trigger and how much it does.
pub fn workflow_summary(w: &Workflow, names: &dyn Fn(&str) -> String) -> String {
    let trigger = w
        .nodes
        .iter()
        .find(|n| n.kind.is_trigger())
        .map(|n| node_label(&n.kind, names))
        .unwrap_or_else(|| "No trigger".into());
    let actions = w.nodes.iter().filter(|n| n.kind.is_action()).count();
    let conditions = w.nodes.iter().filter(|n| n.kind.is_condition()).count();
    let mut parts = vec![trigger];
    if conditions > 0 {
        parts.push(plural(conditions, "condition"));
    }
    parts.push(plural(actions, "action"));
    parts.join(" · ")
}

/// A validation error with the step it's about named, e.g. "Delay must be
/// 1–3600 seconds: “Wait 2 h 0 min”".
pub fn explain(error: &str, w: &Workflow, names: &dyn Fn(&str) -> String) -> String {
    let (message, id) = nyx_home::automation::error_step(error);
    let message = message.trim_start_matches("Invalid workflow: ");
    match id.and_then(|id| w.nodes.iter().find(|n| n.id == id)) {
        Some(node) => format!("{message}: “{}”.", node_label(&node.kind, names)),
        None => format!("{}.", message.trim_end_matches('.')),
    }
}

pub fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

pub fn owner_label(owner: Owner) -> &'static str {
    match owner {
        Owner::Desktop => "Desktop",
        Owner::HomeAssistant => "Home Assistant",
        Owner::HybridDesktop => "Hybrid",
    }
}

/// How a run started, from the trigger the service recorded.
pub fn trigger_label(trigger: &str, names: &dyn Fn(&str) -> String) -> String {
    match trigger {
        "manual" => "Run manually".into(),
        "live_test" => "Live test".into(),
        "startup" => "Desktop started".into(),
        "interval" => "Schedule".into(),
        "window_opened" => "Window opened".into(),
        "window_closed" => "Window closed".into(),
        other => match other.strip_prefix("home:") {
            Some(entity) => format!("{} changed", names(entity)),
            None => sentence(other),
        },
    }
}

/// A run or step status: its label, icon and style class.
pub fn status(status: &str) -> (&'static str, &'static str, &'static str) {
    match status {
        "completed" => ("Completed", "emblem-ok-symbolic", "success"),
        "completed_with_errors" => (
            "Completed with errors",
            "dialog-warning-symbolic",
            "warning",
        ),
        "failed" => ("Failed", "dialog-warning-symbolic", "danger"),
        "running" => ("Running", "media-playback-start-symbolic", "accent"),
        "canceled" => ("Canceled", "process-stop-symbolic", ""),
        "interrupted" => ("Interrupted", "process-stop-symbolic", "warning"),
        "skipped" => ("Skipped", "go-down-symbolic", ""),
        _ => ("Unknown", "dialog-information-symbolic", ""),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use serde_json::json;

    fn entity(id: &str, state: &str, attributes: Value) -> Entity {
        serde_json::from_value(json!({"entity_id": id, "state": state, "attributes": attributes}))
            .unwrap()
    }

    #[test]
    fn states_read_as_words() {
        assert_eq!(
            state_text(&entity(
                "binary_sensor.front",
                "on",
                json!({"device_class": "door"})
            )),
            "Open"
        );
        assert_eq!(
            state_text(&entity(
                "binary_sensor.hall",
                "off",
                json!({"device_class": "motion"})
            )),
            "Clear"
        );
        assert_eq!(
            state_text(&entity(
                "sensor.temp",
                "21.04",
                json!({"unit_of_measurement": "°C"})
            )),
            "21°C"
        );
        assert_eq!(
            state_text(&entity(
                "sensor.power",
                "153.26",
                json!({"unit_of_measurement": "W"})
            )),
            "153.3 W"
        );
        assert_eq!(
            state_text(&entity("person.ken", "not_home", json!({}))),
            "Away"
        );
        assert_eq!(
            state_text(&entity("vacuum.robot", "returning_home", json!({}))),
            "Returning home"
        );
        assert_eq!(
            state_text(&entity("light.desk", "unavailable", json!({}))),
            "Unavailable"
        );
    }

    #[test]
    fn dimmable_lights_report_brightness() {
        let light = entity(
            "light.desk",
            "on",
            json!({"supported_color_modes": ["brightness"], "brightness": 128}),
        );
        assert_eq!(brightness(&light).map(f64::round), Some(50.0));
        assert_eq!(state_text(&light), "On · 50%");
        let plain = entity(
            "light.plug",
            "on",
            json!({"supported_color_modes": ["onoff"]}),
        );
        assert_eq!(brightness(&plain), None);
    }

    #[test]
    fn summary_counts_what_matters() {
        let entities = [
            entity("light.a", "on", json!({})),
            entity("light.b", "off", json!({})),
            entity("binary_sensor.door", "on", json!({"device_class": "door"})),
            entity("person.ken", "home", json!({})),
            entity("climate.hall", "heat", json!({"current_temperature": 20.5})),
        ];
        let s = summarize(&entities);
        assert_eq!((s.lights_on, s.lights, s.open, s.people_home), (1, 2, 1, 1));
        assert_eq!(s.climate.as_deref(), Some("20.5°"));
    }

    #[test]
    fn tiles_drop_the_device_name() {
        let mut e = entity(
            "sensor.purifier_air_quality",
            "excellent",
            json!({"friendly_name": "Air Purifier  Air quality"}),
        );
        e.device_name = Some("Air Purifier".into());
        assert_eq!(short_name(&e), "Air quality");
        let mut lamp = entity("light.lamp", "off", json!({"friendly_name": "BR Lamp-L"}));
        lamp.device_name = Some("BR Lamp-L".into());
        assert_eq!(short_name(&lamp), "BR Lamp-L");
        let mut vacuum = entity(
            "vacuum.z60",
            "docked",
            json!({"friendly_name": "Z60 Ultra  Z60 Ultra"}),
        );
        vacuum.device_name = Some("Z60 Ultra".into());
        assert_eq!(short_name(&vacuum), "Z60 Ultra");
    }

    #[test]
    fn robots_show_their_current_map() {
        let on = |mut e: Entity| {
            e.device_id = Some("vac".into());
            e
        };
        let vacuum = on(entity("vacuum.z60", "docked", json!({})));
        let current = on(entity(
            "camera.z60_map",
            "2026-10-27",
            json!({"friendly_name": "Z60 Current Map"}),
        ));
        let saved = on(entity(
            "camera.z60_map_1",
            "2026-10-27",
            json!({"friendly_name": "Z60 Saved Map 1"}),
        ));
        let all = [vacuum.clone(), saved, current];
        assert_eq!(
            robot_map(&vacuum, &all).map(|e| e.entity_id.as_str()),
            Some("camera.z60_map")
        );
    }

    #[test]
    fn health_counts_a_device_once() {
        let port = |n: u8| {
            let mut e = entity(&format!("switch.port_{n}"), "unavailable", json!({}));
            e.device_id = Some("switch-1".into());
            e.device_name = Some("USW Ultra".into());
            e
        };
        let mut diagnostic = entity("button.restart", "unavailable", json!({}));
        diagnostic.entity_category = Some("config".into());
        let battery = entity("sensor.remote", "12", json!({"device_class": "battery"}));
        let entities = [port(1), port(2), diagnostic, battery];
        let issues = health(&entities);
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].title, "USW Ultra");
        assert_eq!(issues[0].problem, "2 entities unavailable");
        assert!(issues[1].problem.starts_with("Low battery"));
    }

    #[test]
    fn times_read_naturally() {
        assert_eq!(relative_time(1000, 1010), "Just now");
        assert_eq!(relative_time(1000, 1000 + 300), "5 min ago");
        assert_eq!(relative_time(1000, 1000 + 3 * 3600), "3 hours ago");
        assert_eq!(duration(75), "1 min 15 s");
    }

    #[test]
    fn validation_errors_name_the_step() {
        let w: Workflow = serde_json::from_value(json!({
            "schema_version": 1, "id": "w", "name": "W", "enabled": true,
            "nodes": [{"id": "t", "type": "manual"}, {"id": "v", "type": "set_volume", "percent": 150}],
            "edges": [{"from": "t", "to": "v"}]
        }))
        .unwrap();
        let error = w.validate().unwrap_err();
        assert_eq!(
            explain(&error, &w, &|id: &str| id.to_string()),
            "Volume must be 0–100: “Set volume to 150%”."
        );
    }

    #[test]
    fn automations_describe_themselves() {
        let names = |id: &str| {
            if id == "light.office" {
                "Office lights".into()
            } else {
                id.into()
            }
        };
        assert_eq!(
            node_label(
                &NodeKind::SetEntity {
                    entity_id: "light.office".into(),
                    turn_on: true
                },
                &names
            ),
            "Turn on Office lights"
        );
        assert_eq!(
            node_label(&NodeKind::Interval { seconds: 7200 }, &names),
            "Every 2 hours"
        );
        assert_eq!(
            trigger_label("home:light.office", &names),
            "Office lights changed"
        );
    }
}
