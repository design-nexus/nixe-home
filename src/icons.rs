//! Home's own symbolic icons, for things the icon themes have no clear icon
//! for. Written once to the app's cache and added to the icon theme, as the
//! suite's Settings icon is. Filled shapes: GTK fills every path of a
//! symbolic icon.

/// Automations: a trigger branching to two actions.
pub const AUTOMATIONS: &str = "nyx-home-automations-symbolic";

const AUTOMATIONS_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24">
<g fill="#000000">
<circle cx="5" cy="12" r="2.6"/>
<circle cx="19" cy="6" r="2.6"/>
<circle cx="19" cy="18" r="2.6"/>
<rect x="7" y="11.2" width="5" height="1.6" rx="0.8"/>
<rect x="11.2" y="5.2" width="1.6" height="13.6" rx="0.8"/>
<rect x="11.2" y="5.2" width="5.6" height="1.6" rx="0.8"/>
<rect x="11.2" y="17.2" width="5.6" height="1.6" rx="0.8"/>
</g>
</svg>
"##;

/// A robot's map: a folded map of three panels.
pub const MAP: &str = "nyx-home-map-symbolic";

const MAP_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 24 24">
<g fill="#000000">
<path d="M3 5.6 8.4 3.5v15L3 20.6Z"/>
<path d="M9.8 3.5l4.4 2.1v15l-4.4-2.1Z"/>
<path d="M15.6 5.6 21 3.5v15l-5.4 2.1Z"/>
</g>
</svg>
"##;

pub fn install() {
    let dir = nyx_core::paths::cache_dir().join("icons");
    for (name, svg) in [(AUTOMATIONS, AUTOMATIONS_SVG), (MAP, MAP_SVG)] {
        let path = dir.join(format!("{name}.svg"));
        if std::fs::read_to_string(&path).ok().as_deref() != Some(svg) {
            let _ = nyx_core::cmd::atomic_write(&path, svg);
        }
    }
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::IconTheme::for_display(&display).add_search_path(dir);
    }
}
