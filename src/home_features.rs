//! Capability-driven presentation for Home Assistant devices.
use crate::ha::Entity;

pub const DASHBOARD_CARDS: [(&str, &str); 6] = [
    ("summary", "Home status"),
    ("favorites", "Favorites"),
    ("cameras", "Cameras"),
    ("robots", "Robots"),
    ("energy", "Energy"),
    ("health", "Device health"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RobotAction {
    Start,
    Pause,
    Stop,
    Dock,
    Locate,
    CleanSpot,
    CleanArea,
}

impl RobotAction {
    pub fn label(self, domain: &str) -> &'static str {
        match self {
            Self::Start if domain == "lawn_mower" => "Start mowing",
            Self::Start => "Start",
            Self::Pause => "Pause",
            Self::Stop => "Stop",
            Self::Dock => "Dock",
            Self::Locate => "Locate",
            Self::CleanSpot => "Clean spot",
            Self::CleanArea => "Clean area…",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Start => "media-playback-start-symbolic",
            Self::Pause => "media-playback-pause-symbolic",
            Self::Stop => "media-playback-stop-symbolic",
            Self::Dock => "go-home-symbolic",
            Self::Locate => "find-location-symbolic",
            Self::CleanSpot => "mark-location-symbolic",
            Self::CleanArea => "view-grid-symbolic",
        }
    }

    pub fn service(self, domain: &str) -> Option<&'static str> {
        match (domain, self) {
            ("vacuum", Self::Start) => Some("start"),
            ("vacuum", Self::Pause) | ("lawn_mower", Self::Pause) => Some("pause"),
            ("vacuum", Self::Stop) | ("lawn_mower", Self::Stop) => Some("stop"),
            ("vacuum", Self::Dock) => Some("return_to_base"),
            ("lawn_mower", Self::Dock) => Some("dock"),
            ("lawn_mower", Self::Start) => Some("start_mowing"),
            ("vacuum", Self::Locate) => Some("locate"),
            ("vacuum", Self::CleanSpot) => Some("clean_spot"),
            ("vacuum", Self::CleanArea) => Some("clean_area"),
            _ => None,
        }
    }
}

pub fn robot_actions(entity: &Entity) -> Vec<RobotAction> {
    let features = entity.attributes["supported_features"]
        .as_u64()
        .unwrap_or(0);
    let available = !matches!(entity.state.as_str(), "unavailable" | "unknown");
    if !available {
        return Vec::new();
    }
    let flags: &[(u64, RobotAction)] = match entity.domain() {
        // Home Assistant VacuumEntityFeature values.
        "vacuum" => &[
            (8192, RobotAction::Start),
            (4, RobotAction::Pause),
            (8, RobotAction::Stop),
            (16, RobotAction::Dock),
            (512, RobotAction::Locate),
            (1024, RobotAction::CleanSpot),
            (16384, RobotAction::CleanArea),
        ],
        // Home Assistant LawnMowerEntityFeature values.
        "lawn_mower" => &[
            (1, RobotAction::Start),
            (2, RobotAction::Pause),
            (8, RobotAction::Stop),
            (4, RobotAction::Dock),
        ],
        _ => return Vec::new(),
    };
    flags
        .iter()
        .filter_map(|(flag, action)| (features & flag != 0).then_some(*action))
        .collect()
}

pub fn is_energy(entity: &Entity) -> bool {
    entity.domain() == "sensor"
        && matches!(
            entity.attributes["device_class"].as_str(),
            Some("energy" | "power")
        )
}

pub fn health_issue(entity: &Entity) -> Option<String> {
    if entity.state == "unavailable" {
        return Some("Unavailable".into());
    }
    if entity.domain() == "update" && entity.state == "on" {
        return Some("Update available".into());
    }
    if entity.domain() == "sensor"
        && entity.attributes["device_class"] == "battery"
        && entity
            .state
            .parse::<f64>()
            .is_ok_and(|level| (0.0..=20.0).contains(&level))
    {
        return Some(format!("Low battery · {}%", entity.state));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entity(id: &str, state: &str, features: u64) -> Entity {
        serde_json::from_value(json!({
            "entity_id": id,
            "state": state,
            "attributes": {"supported_features": features}
        }))
        .unwrap()
    }

    #[test]
    fn robot_controls_follow_supported_features() {
        assert_eq!(
            robot_actions(&entity("vacuum.test", "docked", 8192 | 16)),
            [RobotAction::Start, RobotAction::Dock]
        );
        assert_eq!(
            robot_actions(&entity("lawn_mower.test", "docked", 1 | 2 | 4)),
            [RobotAction::Start, RobotAction::Pause, RobotAction::Dock]
        );
        assert!(robot_actions(&entity("vacuum.test", "unavailable", 8192)).is_empty());
    }
}
