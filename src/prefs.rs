use nyx_theme::ThemeMode;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub mode: ThemeMode,
    pub theme: String,
    pub glow: bool,
    pub reduce_motion: bool,
    pub sidebar_collapsed: bool,
    pub last_section: String,
    pub home_url: String,
    /// Kept only to move old favorites to their devices.
    pub favorite_entities: Vec<String>,
    /// Devices pinned to Overview, by device ID.
    pub favorite_devices: Vec<String>,
    pub dashboard_order: Vec<String>,
    pub dashboard_hidden: Vec<String>,
    /// Rooms lists diagnostic, configuration and hidden entities, and
    /// devices hidden in Nyx Home, too.
    pub show_all_entities: bool,
    /// Devices hidden in Nyx Home, by device ID.
    pub hidden_devices: Vec<String>,
    /// Light groups (or `room-lights:<room>`) whose lights are hidden in
    /// Nyx Home, so only the group shows.
    pub collapsed_groups: Vec<String>,
    /// Card order per grid (`rooms:light`, `cameras:Outside`, …).
    pub card_order: BTreeMap<String, Vec<String>>,
    /// Section order per page (`rooms`, `cameras`).
    pub section_order: BTreeMap<String, Vec<String>>,
    /// Card sizes chosen in Arrange mode, by device or scene ID (`2x4`).
    pub card_sizes: BTreeMap<String, String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            mode: ThemeMode::Omarchy,
            theme: "tokyo-night".into(),
            glow: true,
            reduce_motion: false,
            sidebar_collapsed: false,
            last_section: "overview".into(),
            home_url: String::new(),
            favorite_entities: Vec::new(),
            favorite_devices: Vec::new(),
            dashboard_order: [
                "summary",
                "favorites",
                "cameras",
                "robots",
                "energy",
                "health",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            dashboard_hidden: Vec::new(),
            show_all_entities: false,
            hidden_devices: Vec::new(),
            collapsed_groups: Vec::new(),
            card_order: BTreeMap::new(),
            section_order: BTreeMap::new(),
            card_sizes: BTreeMap::new(),
        }
    }
}

nyx_core::prefs_store!(Prefs);
