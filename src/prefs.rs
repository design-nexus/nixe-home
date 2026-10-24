use nixe_theme::ThemeMode;
use serde::{Deserialize, Serialize};

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
    pub favorite_entities: Vec<String>,
    pub dashboard_order: Vec<String>,
    pub dashboard_hidden: Vec<String>,
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
        }
    }
}

nixe_core::prefs_store!(Prefs);
