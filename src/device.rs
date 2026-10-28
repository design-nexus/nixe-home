//! Devices as the unit of the interface: Home Assistant entities grouped by
//! device, what kind of device each is, which entity leads it, and where the
//! rest go. Kept free of GTK so it can be tested.
use crate::ha::{DeviceInfo, Entity};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Light,
    Climate,
    Camera,
    Robot,
    Media,
    Fan,
    Security,
    Lock,
    Cover,
    Switch,
    Sensors,
}

impl Kind {
    /// The style class that gives a card its device color.
    pub fn css(self) -> &'static str {
        match self {
            Self::Climate => "kind-climate",
            Self::Light => "kind-light",
            Self::Fan => "kind-fan",
            Self::Media => "kind-media",
            Self::Camera => "kind-camera",
            Self::Robot => "kind-robot",
            Self::Security | Self::Lock => "kind-security",
            Self::Cover => "kind-cover",
            Self::Switch => "kind-switch",
            Self::Sensors => "kind-sensor",
        }
    }

    /// A stable name for saving this kind's layout.
    pub fn key(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Climate => "climate",
            Self::Camera => "camera",
            Self::Robot => "robot",
            Self::Media => "media",
            Self::Fan => "fan",
            Self::Security => "security",
            Self::Lock => "lock",
            Self::Cover => "cover",
            Self::Switch => "switch",
            Self::Sensors => "sensors",
        }
    }

    /// The heading of this kind's section.
    pub fn title(self) -> &'static str {
        match self {
            Self::Light => "Lights",
            Self::Climate => "Climate",
            Self::Camera => "Cameras",
            Self::Robot => "Robots",
            Self::Media => "Media",
            Self::Fan => "Fans and purifiers",
            Self::Security => "Security",
            Self::Lock => "Locks",
            Self::Cover => "Covers",
            Self::Switch => "Switches",
            Self::Sensors => "Sensors",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Climate => "weather-few-clouds-symbolic",
            Self::Light => "display-brightness-symbolic",
            Self::Fan => "weather-windy-symbolic",
            Self::Media => "multimedia-player-symbolic",
            Self::Camera => "camera-web-symbolic",
            Self::Robot => "applications-engineering-symbolic",
            Self::Security => "security-high-symbolic",
            Self::Lock => "changes-prevent-symbolic",
            Self::Cover => "go-down-symbolic",
            Self::Switch => "system-shutdown-symbolic",
            Self::Sensors => "dialog-information-symbolic",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Device {
    /// The registry's device ID, or `entity:<id>` for an entity on its own.
    pub id: String,
    pub name: String,
    pub area: Option<String>,
    pub kind: Kind,
    pub primary: Option<String>,
    /// Every entity of the device, sorted by ID.
    pub entities: Vec<String>,
    pub maker: Option<String>,
    pub model: Option<String>,
    /// For a light group, the lights it holds.
    pub members: Option<Vec<String>>,
    /// Hidden in Nyx Home (shown only with Show every device).
    pub hidden: bool,
    /// A light hidden because its group is set to hide its lights.
    pub grouped: bool,
    /// Marked plugged in (the `plugged_in` label in Home Assistant).
    pub plugged: bool,
}

impl Device {
    /// A light group made in Home Assistant's UI, which Home can edit.
    pub fn group_entry(&self, entities: &BTreeMap<String, Entity>) -> Option<String> {
        self.members.as_ref()?;
        let e = self.primary(entities)?;
        (e.platform.as_deref() == Some("group"))
            .then(|| e.config_entry_id.clone())
            .flatten()
    }
}

/// Lights whose group is set to hide them: the members of each collapsed
/// light group, and every light of each collapsed room (`room-lights:<room>`).
pub fn grouped_lights(devices: &[Device], collapsed: &[String]) -> HashSet<String> {
    let mut out = HashSet::new();
    for id in collapsed {
        if let Some(room) = id.strip_prefix(ROOM_LIGHTS) {
            out.extend(
                devices
                    .iter()
                    .filter(|d| d.kind == Kind::Light && d.members.is_none())
                    .filter(|d| d.area.as_deref() == Some(room))
                    .filter_map(|d| d.primary.clone()),
            );
        } else if let Some(group) = devices.iter().find(|d| d.id == *id) {
            out.extend(group.members.iter().flatten().cloned());
        }
    }
    out
}

/// The ID of a room's automatic light group card.
pub const ROOM_LIGHTS: &str = "room-lights:";

/// Rooms with two or more lights that no light group already covers
/// exactly: `(area, lights)`.
pub fn room_light_sets(devices: &[Device]) -> Vec<(String, Vec<String>)> {
    let mut by_area: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for d in devices {
        if d.kind == Kind::Light
            && d.members.is_none()
            && let (Some(area), Some(light)) = (&d.area, &d.primary)
        {
            by_area.entry(area).or_default().push(light.clone());
        }
    }
    let groups: Vec<Vec<String>> = devices
        .iter()
        .filter_map(|d| d.members.clone())
        .map(|mut m| {
            m.sort();
            m
        })
        .collect();
    by_area
        .into_iter()
        .filter(|(_, lights)| lights.len() >= 2)
        .filter_map(|(area, mut lights)| {
            lights.sort();
            (!groups.contains(&lights)).then(|| (area.to_string(), lights))
        })
        .collect()
}

/// Where an entity goes in a device's details.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Controls,
    Readings,
    Settings,
    Diagnostics,
}

impl Section {
    pub fn title(self) -> &'static str {
        match self {
            Self::Controls => "Controls",
            Self::Readings => "Readings",
            Self::Settings => "Settings",
            Self::Diagnostics => "Diagnostics",
        }
    }
}

/// Entities that never belong on a device card or in its details.
fn ignored(e: &Entity) -> bool {
    matches!(
        e.domain(),
        "zone"
            | "persistent_notification"
            | "conversation"
            | "tts"
            | "stt"
            | "tag"
            | "wake_word"
            | "assist_satellite"
            | "automation"
            | "device_tracker"
            | "notify"
            | "todo"
            | "sun"
    )
}

/// Whether an entity counts toward what a device shows on its card.
pub fn visible(e: &Entity, show_all: bool) -> bool {
    !ignored(e) && (show_all || (e.entity_category.is_none() && !e.hidden))
}

pub fn section(e: &Entity) -> Option<Section> {
    if ignored(e) {
        return None;
    }
    Some(match e.entity_category.as_deref() {
        Some("config") => Section::Settings,
        Some(_) => Section::Diagnostics,
        None if is_control(e) => Section::Controls,
        None => Section::Readings,
    })
}

fn is_control(e: &Entity) -> bool {
    matches!(
        e.domain(),
        "switch"
            | "input_boolean"
            | "light"
            | "fan"
            | "humidifier"
            | "siren"
            | "select"
            | "input_select"
            | "number"
            | "input_number"
            | "button"
            | "input_button"
            | "lock"
            | "cover"
            | "scene"
            | "script"
            | "media_player"
            | "climate"
            | "alarm_control_panel"
            | "vacuum"
            | "lawn_mower"
    )
}

/// The kind a device is, decided by the most important entity it has.
const RANKING: &[(Kind, &[&str])] = &[
    (Kind::Robot, &["vacuum", "lawn_mower"]),
    (Kind::Camera, &["camera"]),
    (Kind::Security, &["alarm_control_panel"]),
    (Kind::Climate, &["climate", "water_heater"]),
    (Kind::Light, &["light"]),
    (Kind::Media, &["media_player"]),
    (Kind::Fan, &["fan", "humidifier"]),
    (Kind::Lock, &["lock"]),
    (Kind::Cover, &["cover"]),
    (
        Kind::Switch,
        &[
            "switch",
            "input_boolean",
            "siren",
            "select",
            "number",
            "button",
            "input_button",
        ],
    ),
    (
        Kind::Sensors,
        &[
            "sensor",
            "binary_sensor",
            "weather",
            "person",
            "calendar",
            "event",
            "image",
        ],
    ),
];

fn classify<'a>(entities: &[&'a Entity]) -> Option<(Kind, &'a Entity)> {
    RANKING.iter().find_map(|(kind, domains)| {
        entities
            .iter()
            .find(|e| domains.contains(&e.domain()))
            .map(|e| (*kind, *e))
    })
}

/// Scenes and scripts on their own (not part of a device), by area.
pub fn scenes(entities: &BTreeMap<String, Entity>) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in entities.values() {
        if matches!(e.domain(), "scene" | "script") && e.device_id.is_none() && !e.hidden {
            out.entry(e.area_name.clone().unwrap_or_else(|| "Other".into()))
                .or_default()
                .push(e.entity_id.clone());
        }
    }
    out
}

/// Every device worth a card. A device with nothing visible (only network
/// trackers, only diagnostics) gets none unless `show_all` is set.
pub fn group(
    entities: &BTreeMap<String, Entity>,
    devices: &HashMap<String, DeviceInfo>,
    show_all: bool,
) -> Vec<Device> {
    let mut by_device: BTreeMap<String, Vec<&Entity>> = BTreeMap::new();
    for e in entities.values() {
        if ignored(e) {
            continue;
        }
        match &e.device_id {
            Some(id) => by_device.entry(id.clone()).or_default().push(e),
            None if matches!(e.domain(), "scene" | "script") => {}
            None => by_device
                .entry(format!("entity:{}", e.entity_id))
                .or_default()
                .push(e),
        }
    }
    let mut out = Vec::new();
    for (id, members) in by_device {
        let shown: Vec<&Entity> = members
            .iter()
            .copied()
            .filter(|e| visible(e, false))
            .collect();
        let deciding = if shown.is_empty() && show_all {
            members.clone()
        } else {
            shown
        };
        let Some((kind, primary)) = classify(&deciding) else {
            continue;
        };
        let info = devices.get(&id);
        let name = info
            .map(|i| i.name.clone())
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| primary.name());
        out.push(Device {
            name: name.split_whitespace().collect::<Vec<_>>().join(" "),
            area: info
                .and_then(|i| i.area_name.clone())
                .or_else(|| primary.area_name.clone()),
            kind,
            primary: Some(primary.entity_id.clone()),
            entities: members.iter().map(|e| e.entity_id.clone()).collect(),
            maker: info.and_then(|i| i.manufacturer.clone()),
            model: info.and_then(|i| i.model.clone()),
            members: primary.members(),
            hidden: false,
            grouped: false,
            plugged: info.is_some_and(|i| i.labels.iter().any(|l| l == crate::ha::PLUGGED_IN)),
            id,
        });
    }
    out.sort_by_key(|d| (d.kind, d.name.to_lowercase()));
    out
}

impl Device {
    /// The first of this device's entities that `pred` accepts.
    pub fn find<'a>(
        &self,
        entities: &'a BTreeMap<String, Entity>,
        pred: impl Fn(&Entity) -> bool,
    ) -> Option<&'a Entity> {
        self.entities
            .iter()
            .filter_map(|id| entities.get(id))
            .find(|e| pred(e))
    }

    pub fn primary<'a>(&self, entities: &'a BTreeMap<String, Entity>) -> Option<&'a Entity> {
        self.primary.as_ref().and_then(|id| entities.get(id))
    }

    /// Readings worth showing on a card: main sensors with a value, the
    /// familiar kinds (temperature, humidity, battery, power) first.
    pub fn readings<'a>(
        &self,
        entities: &'a BTreeMap<String, Entity>,
        n: usize,
    ) -> Vec<&'a Entity> {
        let rank = |e: &Entity| match e.attributes["device_class"].as_str() {
            Some("temperature" | "humidity" | "battery" | "power" | "energy" | "weight") => 0,
            Some("aqi" | "pm25" | "pm10" | "carbon_dioxide" | "illuminance") => 0,
            Some(_) => 1,
            None if e.attributes["unit_of_measurement"].is_string() => 1,
            None => 2,
        };
        let mut found: Vec<&Entity> = self
            .entities
            .iter()
            .filter_map(|id| entities.get(id))
            .filter(|e| {
                matches!(
                    e.domain(),
                    "sensor" | "binary_sensor" | "weather" | "person"
                ) && visible(e, false)
                    && !matches!(e.state.as_str(), "unknown" | "unavailable")
            })
            .collect();
        found.sort_by_key(|e| (rank(e), e.entity_id.clone()));
        found.truncate(n);
        found
    }

    /// Whether `query` (lower case) matches the device or any of its entities.
    pub fn matches(&self, query: &str, entities: &BTreeMap<String, Entity>) -> bool {
        query.is_empty()
            || self.name.to_lowercase().contains(query)
            || self
                .area
                .as_deref()
                .is_some_and(|a| a.to_lowercase().contains(query))
            || self.entities.iter().any(|id| {
                id.contains(query)
                    || entities
                        .get(id)
                        .is_some_and(|e| e.name().to_lowercase().contains(query))
            })
    }
}

pub fn device_class(e: &Entity) -> &str {
    e.attributes["device_class"].as_str().unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entity(id: &str, state: &str, attrs: serde_json::Value, device: Option<&str>) -> Entity {
        let mut e: Entity =
            serde_json::from_value(json!({"entity_id": id, "state": state, "attributes": attrs}))
                .unwrap();
        e.device_id = device.map(str::to_string);
        e
    }

    fn map(list: Vec<Entity>) -> BTreeMap<String, Entity> {
        list.into_iter().map(|e| (e.entity_id.clone(), e)).collect()
    }

    #[test]
    fn devices_take_the_kind_of_their_main_entity() {
        let mut battery = entity(
            "sensor.door_battery",
            "90",
            json!({"device_class": "battery"}),
            Some("door"),
        );
        battery.entity_category = Some("diagnostic".into());
        let entities = map(vec![
            entity("camera.doorbell", "idle", json!({}), Some("door")),
            entity(
                "binary_sensor.doorbell_motion",
                "off",
                json!({"device_class": "motion"}),
                Some("door"),
            ),
            entity("siren.doorbell", "off", json!({}), Some("door")),
            battery,
            entity(
                "alarm_control_panel.base",
                "armed_home",
                json!({}),
                Some("base"),
            ),
            entity("climate.main_floor", "heat_cool", json!({}), None),
            entity("device_tracker.phone", "home", json!({}), Some("phone")),
            entity("scene.movie", "scening", json!({}), None),
        ]);
        let devices = HashMap::from([(
            "door".to_string(),
            DeviceInfo {
                id: "door".into(),
                name: "Doorbell".into(),
                area_name: Some("Outside".into()),
                ..Default::default()
            },
        )]);
        let found = group(&entities, &devices, false);
        let kinds: Vec<_> = found.iter().map(|d| (d.name.as_str(), d.kind)).collect();
        // The tracker-only phone gets no card; entities on their own are named by their ID here.
        assert_eq!(
            kinds,
            [
                ("climate.main_floor", Kind::Climate),
                ("Doorbell", Kind::Camera),
                ("alarm_control_panel.base", Kind::Security),
            ]
        );
        let doorbell = found.iter().find(|d| d.kind == Kind::Camera).unwrap();
        assert_eq!(doorbell.primary.as_deref(), Some("camera.doorbell"));
        assert_eq!(doorbell.area.as_deref(), Some("Outside"));
        assert_eq!(doorbell.entities.len(), 4);
        assert!(
            doorbell
                .find(&entities, |e| device_class(e) == "battery")
                .is_some()
        );
        assert_eq!(scenes(&entities)["Other"], ["scene.movie"]);
    }

    #[test]
    fn rooms_get_a_light_set_unless_a_group_covers_it() {
        let mut a = entity("light.a", "on", json!({}), None);
        a.area_name = Some("Bedroom".into());
        let mut b = entity("light.b", "off", json!({}), None);
        b.area_name = Some("Bedroom".into());
        let mut c = entity("light.c", "off", json!({}), None);
        c.area_name = Some("Office".into());
        let devices = group(
            &map(vec![a.clone(), b.clone(), c.clone()]),
            &HashMap::new(),
            false,
        );
        assert_eq!(
            room_light_sets(&devices),
            [(
                "Bedroom".to_string(),
                vec!["light.a".to_string(), "light.b".to_string()]
            )]
        );
        let grp = entity(
            "light.bedroom",
            "on",
            json!({"entity_id": ["light.b", "light.a"]}),
            None,
        );
        let devices = group(&map(vec![a, b, c, grp]), &HashMap::new(), false);
        assert!(room_light_sets(&devices).is_empty());
        let group_card = devices.iter().find(|d| d.members.is_some()).unwrap();
        assert_eq!(group_card.kind, Kind::Light);
        let hidden = grouped_lights(&devices, std::slice::from_ref(&group_card.id));
        assert!(hidden.contains("light.a") && hidden.contains("light.b"));
        let by_room = grouped_lights(&devices, &[format!("{ROOM_LIGHTS}Office")]);
        assert_eq!(by_room.into_iter().collect::<Vec<_>>(), ["light.c"]);
    }

    #[test]
    fn diagnostics_only_devices_appear_when_showing_everything() {
        let mut uptime = entity("sensor.switch_uptime", "5", json!({}), Some("usw"));
        uptime.entity_category = Some("diagnostic".into());
        let entities = map(vec![uptime]);
        assert!(group(&entities, &HashMap::new(), false).is_empty());
        assert_eq!(
            group(&entities, &HashMap::new(), true)[0].kind,
            Kind::Sensors
        );
    }

    #[test]
    fn details_sections_follow_category_and_domain() {
        let mut setting = entity("select.vacuum_suction", "max", json!({}), Some("v"));
        setting.entity_category = Some("config".into());
        assert_eq!(section(&setting), Some(Section::Settings));
        assert_eq!(
            section(&entity("select.vacuum_mode", "sweep", json!({}), Some("v"))),
            Some(Section::Controls)
        );
        assert_eq!(
            section(&entity("sensor.vacuum_area", "40", json!({}), Some("v"))),
            Some(Section::Readings)
        );
        assert_eq!(
            section(&entity("device_tracker.x", "home", json!({}), Some("v"))),
            None
        );
    }

    #[test]
    fn readings_prefer_familiar_measurements() {
        let entities = map(vec![
            entity("sensor.p_mode", "auto", json!({}), Some("p")),
            entity(
                "sensor.p_pm25",
                "3",
                json!({"device_class": "pm25", "unit_of_measurement": "µg/m³"}),
                Some("p"),
            ),
            entity(
                "sensor.p_unknown",
                "unknown",
                json!({"device_class": "temperature"}),
                Some("p"),
            ),
            entity("fan.p", "on", json!({}), Some("p")),
        ]);
        let device = &group(&entities, &HashMap::new(), false)[0];
        assert_eq!(device.kind, Kind::Fan);
        let ids: Vec<_> = device
            .readings(&entities, 4)
            .iter()
            .map(|e| e.entity_id.as_str())
            .collect();
        assert_eq!(ids, ["sensor.p_pm25", "sensor.p_mode"]);
    }
}
