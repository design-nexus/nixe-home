//! The window: header, sidebar, pages and status bar, toasts and keys, and the
//! Home Assistant state every page reads.
use crate::{
    activity, automations, connection, connections, ha, overview, prefs, rooms, rt, settings, theme,
};
use gtk::{gdk, glib, prelude::*};
use nyx_ui::{chrome, widgets};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    time::Duration,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section {
    Overview,
    Rooms,
    Automations,
    Activity,
    Connections,
}

impl Section {
    pub const ALL: [Self; 5] = [
        Self::Overview,
        Self::Rooms,
        Self::Automations,
        Self::Activity,
        Self::Connections,
    ];
    pub fn id(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Rooms => "rooms",
            Self::Automations => "automations",
            Self::Activity => "activity",
            Self::Connections => "connections",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Rooms => "Rooms",
            Self::Automations => "Automations",
            Self::Activity => "Activity",
            Self::Connections => "Connections",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Overview => "view-grid-symbolic",
            Self::Rooms => "user-home-symbolic",
            Self::Automations => crate::icons::AUTOMATIONS,
            Self::Activity => "document-open-recent-symbolic",
            Self::Connections => "network-server-symbolic",
        }
    }
    fn description(self) -> &'static str {
        match self {
            Self::Overview => "Your home at a glance",
            Self::Rooms => "Every device, by room",
            Self::Automations => "Desktop and Home Assistant automations",
            Self::Activity => "What your automations did",
            Self::Connections => "Home Assistant and the desktop service",
        }
    }
    pub fn parse(id: &str) -> Self {
        match id {
            "home" => Self::Rooms,
            "system" | "integrations" => Self::Connections,
            _ => Self::ALL
                .into_iter()
                .find(|s| s.id() == id)
                .unwrap_or(Self::Overview),
        }
    }
}

/// The state of the link to Home Assistant.
#[derive(Clone, Debug, PartialEq)]
pub enum Link {
    /// No server has been set up.
    NotSetUp,
    /// Reading the saved token from the keyring.
    Unlocking,
    Connecting,
    Online,
    /// Was connected; the live stream dropped and is retrying.
    Offline(String),
    /// A server is set up but no token is saved.
    NeedsToken,
    /// The keyring is locked or missing.
    Locked(String),
    /// Home Assistant refused the token.
    Rejected,
    /// The first connection failed; retrying.
    Unreachable(String),
}

impl Link {
    pub fn label(&self) -> String {
        match self {
            Self::NotSetUp => "Not connected".into(),
            Self::Unlocking => "Unlocking the keyring…".into(),
            Self::Connecting => "Connecting…".into(),
            Self::Online => "Connected".into(),
            Self::Offline(_) => "Offline · reconnecting".into(),
            Self::NeedsToken => "Add an access token to connect".into(),
            Self::Locked(_) => "Keyring locked".into(),
            Self::Rejected => "Token rejected".into(),
            Self::Unreachable(_) => "Can't reach Home Assistant · retrying".into(),
        }
    }
    /// `success`, `warning`, `danger`, or none, for the status dot.
    pub fn tone(&self) -> &'static str {
        match self {
            Self::Online => "success",
            Self::Connecting | Self::Unlocking | Self::Offline(_) | Self::Unreachable(_) => {
                "warning"
            }
            Self::Locked(_) | Self::Rejected | Self::NeedsToken => "danger",
            Self::NotSetUp => "",
        }
    }
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Offline(m) | Self::Locked(m) | Self::Unreachable(m) => Some(m),
            _ => None,
        }
    }
}

pub struct Home {
    pub base: String,
    pub token: String,
    pub link: Link,
    pub entities: BTreeMap<String, ha::Entity>,
    pub areas: Vec<ha::Area>,
    pub device_count: usize,
    pub info: ha::Info,
    pub previews: BTreeMap<String, gdk::Texture>,
}

impl Home {
    /// Home Assistant data is loaded (it may be offline right now).
    pub fn loaded(&self) -> bool {
        !self.entities.is_empty()
    }
    pub fn name_of(&self, entity_id: &str) -> String {
        self.entities
            .get(entity_id)
            .map(ha::Entity::name)
            .unwrap_or_else(|| entity_id.to_string())
    }
    pub fn clear(&mut self) {
        self.entities.clear();
        self.areas.clear();
        self.device_count = 0;
        self.info = ha::Info::default();
        self.previews.clear();
    }
}

pub struct App {
    pub window: gtk::ApplicationWindow,
    pub overlay: gtk::Overlay,
    header: chrome::AppHeader,
    pub search: gtk::SearchEntry,
    stack: gtk::Stack,
    nav: gtk::Box,
    nav_items: Vec<(Section, gtk::Button)>,
    compact_items: Vec<gtk::Widget>,
    collapse: gtk::Button,
    actions: Vec<(Section, gtk::Box)>,
    foot: gtk::Button,
    dot: gtk::Label,
    foot_title: gtk::Label,
    foot_detail: gtk::Label,
    toast: RefCell<Option<gtk::Revealer>>,
    pub home: RefCell<Home>,
    section: Cell<Section>,
    pub overview: overview::Page,
    pub rooms: rooms::Page,
    pub automations: automations::Page,
    pub activity: activity::Page,
    pub connections: connections::Page,
    changed: RefCell<BTreeSet<String>>,
    rebuild: Cell<bool>,
    flush_scheduled: Cell<bool>,
}

thread_local! {
    static APP: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

pub fn get() -> Option<Rc<App>> {
    APP.with(|a| a.borrow().clone())
}

pub fn present(application: &gtk::Application) {
    if let Some(app) = get() {
        app.window.present();
        return;
    }
    theme::install();
    crate::icons::install();
    let prefs = prefs::get();
    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("Home")
        .default_width(1200)
        .default_height(800)
        .build();
    window.add_css_class("home-window");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    window.set_icon_name(Some("io.github.design_nexus.nyx.Home"));
    nyx_ui::keys::install(
        &window,
        nyx_ui::keys::Common {
            settings: Some(open_settings),
            shortcuts: Some(show_shortcuts),
            close: true,
            palette: None,
        },
    );

    let header = chrome::app_header(&window, "user-home-symbolic", "Home");
    let collapse = widgets::bar_button("sidebar-show-symbolic", "Collapse the sidebar (Ctrl+B)");
    collapse.update_property(&[gtk::accessible::Property::Label("Collapse the sidebar")]);
    collapse.connect_clicked(|_| toggle_sidebar());
    header.start.append(&collapse);
    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search devices  (Ctrl+F)"));
    search.add_css_class("bar-search");
    search.set_width_chars(30);
    search.update_property(&[gtk::accessible::Property::Label("Search devices")]);
    header.center.append(&search);

    let overview = overview::Page::new();
    let rooms = rooms::Page::new();
    let automations = automations::Page::new();
    let activity = activity::Page::new();
    let connections = connections::Page::new();
    let mut actions = Vec::new();
    for (section, page_actions) in [
        (Section::Overview, &overview.actions),
        (Section::Rooms, &rooms.actions),
        (Section::Automations, &automations.actions),
        (Section::Activity, &activity.actions),
        (Section::Connections, &connections.actions),
    ] {
        page_actions.set_visible(false);
        header.end.append(page_actions);
        actions.push((section, page_actions.clone()));
    }
    header.end.append(&chrome::settings_button(open_settings));

    let nav = gtk::Box::new(gtk::Orientation::Vertical, 2);
    nav.add_css_class("settings-navigation");
    nav.set_hexpand(false);
    let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let mut nav_items = Vec::new();
    let mut compact_items: Vec<gtk::Widget> = Vec::new();
    for section in Section::ALL {
        if section == Section::Connections {
            let group = widgets::label("SETUP", "nav-group");
            compact_items.push(group.clone().upcast());
            list.append(&group);
        }
        let button = gtk::Button::new();
        button.add_css_class("nav-item");
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        content.append(&gtk::Image::from_icon_name(section.icon()));
        let label = widgets::label(section.title(), "nav-label");
        label.set_hexpand(true);
        compact_items.push(label.clone().upcast());
        content.append(&label);
        button.set_child(Some(&content));
        button.set_tooltip_text(Some(section.description()));
        button.update_property(&[gtk::accessible::Property::Label(section.title())]);
        button.connect_clicked(move |_| show(section));
        list.append(&button);
        nav_items.push((section, button));
    }
    let nav_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .vexpand(true)
        .child(&list)
        .build();
    nav.append(&nav_scroll);

    // The Home Assistant connection, at the sidebar's foot like an account.
    let foot = gtk::Button::new();
    foot.add_css_class("nav-item");
    foot.add_css_class("sidebar-foot");
    let foot_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let dot = widgets::label("●", "status-dot");
    dot.set_accessible_role(gtk::AccessibleRole::Presentation);
    foot_row.append(&dot);
    let foot_text = widgets::vbox(1);
    foot_text.set_hexpand(true);
    let foot_title = widgets::label("Home Assistant", "foot-title");
    foot_title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let foot_detail = widgets::label("", "foot-detail");
    foot_detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
    foot_text.append(&foot_title);
    foot_text.append(&foot_detail);
    compact_items.push(foot_text.clone().upcast());
    foot_row.append(&foot_text);
    foot.set_child(Some(&foot_row));
    foot.connect_clicked(|_| show(Section::Connections));
    nav.append(&foot);

    let stack = gtk::Stack::new();
    stack.add_css_class("settings-content");
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(if prefs.reduce_motion { 0 } else { 150 });
    stack.add_named(&overview.root, Some(Section::Overview.id()));
    stack.add_named(&rooms.root, Some(Section::Rooms.id()));
    stack.add_named(&automations.root, Some(Section::Automations.id()));
    stack.add_named(&activity.root, Some(Section::Activity.id()));
    stack.add_named(&connections.root, Some(Section::Connections.id()));

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.set_vexpand(true);
    body.append(&nav);
    body.append(&stack);

    let frame = widgets::vbox(0);
    frame.add_css_class("window-frame");
    frame.append(&header.root);
    frame.append(&body);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&frame));
    window.set_child(Some(&overlay));

    let link = if prefs.home_url.is_empty() {
        Link::NotSetUp
    } else {
        Link::Unlocking
    };
    let app = Rc::new(App {
        window: window.clone(),
        overlay: overlay.clone(),
        header,
        search: search.clone(),
        stack,
        nav,
        nav_items,
        compact_items,
        collapse,
        actions,
        foot,
        dot,
        foot_title,
        foot_detail,
        toast: RefCell::new(None),
        home: RefCell::new(Home {
            base: prefs.home_url.clone(),
            token: String::new(),
            link,
            entities: BTreeMap::new(),
            areas: Vec::new(),
            device_count: 0,
            info: ha::Info::default(),
            previews: BTreeMap::new(),
        }),
        section: Cell::new(Section::Overview),
        overview,
        rooms,
        automations,
        activity,
        connections,
        changed: RefCell::new(BTreeSet::new()),
        rebuild: Cell::new(true),
        flush_scheduled: Cell::new(false),
    });
    APP.with(|slot| *slot.borrow_mut() = Some(app.clone()));

    search.connect_search_changed(|entry| {
        if let Some(app) = get() {
            app.rooms.set_query(&entry.text(), &app.home.borrow());
        }
    });
    install_keys(&window);
    watch_width(&window, &overlay);
    flush_now();
    // Developer aid: NYX_HOME_PAGE=rooms opens on that page.
    let start = std::env::var("NYX_HOME_PAGE")
        .unwrap_or_else(|_| nyx_theme::start_page("overview", &prefs.last_section));
    show(Section::parse(&start));
    window.present();
    connection::restore();
    // Developer aid: NYX_HOME_OPEN=canvas (or builder, new, connect, settings,
    // shortcuts) opens it once the window is up.
    if let Ok(what) = std::env::var("NYX_HOME_OPEN") {
        glib::timeout_add_local_once(Duration::from_millis(1500), move || match what.as_str() {
            "canvas" => automations::open_canvas(automations::starter_desktop()),
            "builder" => {
                if let Some(app) = get() {
                    crate::builder::present(
                        app.window.upcast_ref(),
                        None,
                        nyx_home::automation::Owner::Desktop,
                        automations::context(),
                        Rc::new(|| {}),
                    );
                }
            }
            "new" => automations::new_automation(),
            "connect" => connection::dialog(),
            "settings" => open_settings(),
            "shortcuts" => show_shortcuts(),
            _ => {}
        });
    }
}

pub fn show(section: Section) {
    let Some(app) = get() else { return };
    app.section.set(section);
    app.stack.set_visible_child_name(section.id());
    app.header.set_subtitle(section.title());
    chrome::set_title(&app.window, "Home", section.title());
    for (candidate, button) in &app.nav_items {
        if *candidate == section {
            button.add_css_class("active");
        } else {
            button.remove_css_class("active");
        }
    }
    for (candidate, actions) in &app.actions {
        actions.set_visible(*candidate == section && actions.first_child().is_some());
    }
    app.search.set_visible(section == Section::Rooms);
    prefs::update(|p| p.last_section = section.id().into());
    match section {
        Section::Automations => app.automations.reload(),
        Section::Activity => app.activity.reload(),
        Section::Connections => app.connections.reload_service(),
        _ => {}
    }
}

pub fn current() -> Section {
    get().map_or(Section::Overview, |app| app.section.get())
}

fn refresh_page() {
    let Some(app) = get() else { return };
    match app.section.get() {
        Section::Automations => app.automations.reload(),
        Section::Activity => app.activity.reload(),
        Section::Connections => app.connections.reload_service(),
        Section::Overview | Section::Rooms => connection::reconnect(),
    }
}

/// Change the link state and show it everywhere.
pub fn set_link(link: Link) {
    let Some(app) = get() else { return };
    app.home.borrow_mut().link = link;
    app.rebuild.set(true);
    schedule_flush();
}

/// An entity changed; its tiles are updated on the next flush.
pub fn entity_changed(entity_id: &str) {
    let Some(app) = get() else { return };
    app.changed.borrow_mut().insert(entity_id.to_string());
    schedule_flush();
}

/// Rebuild every page's layout on the next flush (new entities, a new
/// connection, a setting that changes what is shown).
pub fn rebuild() {
    let Some(app) = get() else { return };
    app.rebuild.set(true);
    schedule_flush();
}

fn schedule_flush() {
    let Some(app) = get() else { return };
    if app.flush_scheduled.replace(true) {
        return;
    }
    glib::timeout_add_local_once(Duration::from_millis(200), flush_now);
}

/// Bring every page up to date with the Home Assistant state, at most a few
/// times a second however fast events arrive.
fn flush_now() {
    let Some(app) = get() else { return };
    app.flush_scheduled.set(false);
    let changed = std::mem::take(&mut *app.changed.borrow_mut());
    let rebuild = app.rebuild.replace(false);
    let home = app.home.borrow();
    let changed = (!rebuild).then_some(&changed);
    app.overview.refresh(&home, changed);
    app.rooms.refresh(&home, changed);
    app.connections.refresh(&home);
    if rebuild {
        app.automations.home_changed(&home);
    }
    let title = if home.info.location_name.is_empty() {
        "Home Assistant".to_string()
    } else {
        home.info.location_name.clone()
    };
    let detail = match (&home.link, home.loaded()) {
        (Link::Online, true) => format!(
            "Connected · {} · {}",
            crate::present::plural(home.areas.len(), "area"),
            crate::present::plural(home.device_count, "device"),
        ),
        (link, _) => link.label(),
    };
    app.foot_title.set_text(&title);
    app.foot_detail.set_text(&detail);
    let summary = format!("{title}: {detail}");
    app.foot
        .set_tooltip_text(Some(&format!("{summary}. Open Connections.")));
    app.foot
        .update_property(&[gtk::accessible::Property::Label(&summary)]);
    for tone in ["success", "warning", "danger"] {
        app.dot.remove_css_class(tone);
    }
    if !home.link.tone().is_empty() {
        app.dot.add_css_class(home.link.tone());
    }
}

/// Call a Home Assistant action on an entity. On failure the tile is put back
/// to the entity's real state and the error is shown.
pub fn call(entity_id: &str, service: &str, data: serde_json::Value) {
    let Some(app) = get() else { return };
    let (base, token) = {
        let home = app.home.borrow();
        (home.base.clone(), home.token.clone())
    };
    if token.is_empty() {
        toast("Connect Home Assistant first.");
        entity_changed(entity_id);
        return;
    }
    let id = entity_id.to_string();
    let service = service.to_string();
    rt::spawn(
        async move {
            let result = ha::call(&base, &token, &id, &service, data).await;
            (id, result)
        },
        |(id, result)| {
            if let Err(error) = result {
                toast(&error);
                entity_changed(&id);
            }
        },
    );
}

pub fn toast(message: &str) {
    show_toast(message, None);
}

pub fn toast_action(message: &str, button: &str, action: impl Fn() + 'static) {
    show_toast(message, Some((button, Box::new(action))));
}

type ToastAction<'a> = Option<(&'a str, Box<dyn Fn()>)>;

fn show_toast(message: &str, action: ToastAction) {
    let Some(app) = get() else {
        eprintln!("nyx-home: {message}");
        return;
    };
    if let Some(old) = app.toast.borrow_mut().take() {
        app.overlay.remove_overlay(&old);
    }
    let label = gtk::Label::new(Some(message));
    label.set_wrap(true);
    label.set_max_width_chars(70);
    let bx = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    bx.add_css_class("toast");
    bx.append(&label);
    let motion = !prefs::get().reduce_motion;
    let has_action = action.is_some();
    let revealer = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideUp)
        .transition_duration(if motion { 180 } else { 0 })
        .child(&bx)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::End)
        .margin_bottom(16)
        .can_target(has_action)
        .build();
    revealer.set_accessible_role(gtk::AccessibleRole::Alert);
    revealer.update_property(&[gtk::accessible::Property::Label(message)]);
    if let Some((text, run)) = action {
        let b = gtk::Button::with_label(text);
        b.add_css_class("toast-action");
        b.set_valign(gtk::Align::Center);
        let r = revealer.clone();
        b.connect_clicked(move |_| {
            run();
            dismiss_toast(&r, motion);
        });
        bx.append(&b);
    }
    app.overlay.add_overlay(&revealer);
    app.overlay.set_measure_overlay(&revealer, false);
    revealer.set_reveal_child(true);
    *app.toast.borrow_mut() = Some(revealer.clone());
    let shown = if has_action { 7000 } else { 3500 };
    glib::timeout_add_local_once(Duration::from_millis(shown), move || {
        dismiss_toast(&revealer, motion)
    });
}

fn dismiss_toast(revealer: &gtk::Revealer, motion: bool) {
    revealer.set_reveal_child(false);
    let revealer = revealer.clone();
    glib::timeout_add_local_once(
        Duration::from_millis(if motion { 200 } else { 0 }),
        move || {
            let Some(app) = get() else { return };
            let current = app.toast.borrow().as_ref() == Some(&revealer);
            if current {
                app.toast.borrow_mut().take();
                app.overlay.remove_overlay(&revealer);
            }
        },
    );
}

fn open_settings() {
    if let Some(app) = get() {
        settings::open(&app.overlay, &app.window);
    }
}

const SHORTCUTS: &[(&[&str], &str)] = &[
    (
        &["Ctrl", "1–5"],
        "Go to Overview, Rooms, Automations, Activity or Connections",
    ),
    (&["Ctrl", "F"], "Search devices"),
    (&["Ctrl", "N"], "New automation"),
    (&["Ctrl", "L"], "Connect to Home Assistant"),
    (&["Ctrl", "B"], "Collapse or expand the sidebar"),
    (&["F5"], "Refresh the page"),
    (&["Enter / Space"], "Use a device tile's main control"),
    (&["Shift", "F10"], "A device's details and options"),
    (&["Ctrl", ","], "Settings"),
    (&["F1"], "Show these shortcuts (or ?)"),
    (&["Esc"], "Clear the search or close what's open"),
    (&["Ctrl", "Q"], "Close the window (Ctrl+W too)"),
];

pub fn show_shortcuts() {
    let (dialog, card) = widgets::dialog("Keyboard shortcuts", 540);
    let list = widgets::vbox(0);
    list.add_css_class("group-list");
    for (keys, what) in SHORTCUTS {
        list.append(&widgets::row(
            what,
            "",
            Some(widgets::key_caps(keys).upcast_ref()),
        ));
    }
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .propagate_natural_height(true)
        .max_content_height(560)
        .child(&list)
        .build();
    card.append(&scroll);
    let close = gtk::Button::with_label("Close");
    close.set_halign(gtk::Align::End);
    let d = dialog.clone();
    close.connect_clicked(move |_| d.close());
    card.append(&close);
    dialog.present();
}

fn install_keys(window: &gtk::ApplicationWindow) {
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed(|_, key, _, mods| {
        let Some(app) = get() else {
            return glib::Propagation::Proceed;
        };
        let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
        let section = match key {
            gdk::Key::_1 => Some(Section::Overview),
            gdk::Key::_2 => Some(Section::Rooms),
            gdk::Key::_3 => Some(Section::Automations),
            gdk::Key::_4 => Some(Section::Activity),
            gdk::Key::_5 => Some(Section::Connections),
            _ => None,
        };
        match key {
            _ if ctrl && section.is_some() => show(section.unwrap_or(Section::Overview)),
            gdk::Key::f if ctrl => {
                show(Section::Rooms);
                app.search.grab_focus();
            }
            gdk::Key::n if ctrl => {
                show(Section::Automations);
                automations::new_automation();
            }
            gdk::Key::l if ctrl => connection::dialog(),
            gdk::Key::b if ctrl => toggle_sidebar(),
            gdk::Key::F1 => show_shortcuts(),
            gdk::Key::F5 => refresh_page(),
            gdk::Key::Escape if nyx_ui::settings_card::is_open() => nyx_ui::settings_card::escape(),
            gdk::Key::Escape if !app.search.text().is_empty() => app.search.set_text(""),
            gdk::Key::Escape if app.overview.editing() => app.overview.set_editing(false),
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
}

const NARROW_BELOW: i32 = 980;

fn watch_width(window: &gtk::ApplicationWindow, overlay: &gtk::Overlay) {
    window.connect_realize(|_| apply_width());
    // Tiled windows are resized by the compositor: a layer over the window
    // that takes no input reports each real size change.
    let probe = gtk::DrawingArea::new();
    probe.set_can_target(false);
    probe.set_can_focus(false);
    probe.connect_resize(|_, _, _| {
        glib::idle_add_local_once(apply_width);
    });
    overlay.add_overlay(&probe);
    overlay.set_measure_overlay(&probe, false);
}

fn apply_width() {
    let Some(app) = get() else { return };
    let width = if app.window.width() > 0 {
        app.window.width()
    } else {
        app.window.default_width()
    };
    if width <= 0 {
        return;
    }
    apply_compact(width < NARROW_BELOW || prefs::get().sidebar_collapsed);
    if nyx_ui::settings_card::is_open() {
        nyx_ui::settings_card::fit(&app.window);
    }
}

fn apply_compact(compact: bool) {
    let Some(app) = get() else { return };
    if compact == app.nav.has_css_class("compact") {
        return;
    }
    if compact {
        app.nav.add_css_class("compact");
    } else {
        app.nav.remove_css_class("compact");
    }
    for widget in &app.compact_items {
        widget.set_visible(!compact);
    }
    for button in app.nav_items.iter().map(|(_, b)| b).chain([&app.foot]) {
        if let Some(content) = button.child() {
            content.set_halign(if compact {
                gtk::Align::Center
            } else {
                gtk::Align::Fill
            });
        }
    }
    app.collapse.set_tooltip_text(Some(if compact {
        "Expand the sidebar (Ctrl+B)"
    } else {
        "Collapse the sidebar (Ctrl+B)"
    }));
}

pub fn toggle_sidebar() {
    prefs::update(|p| p.sidebar_collapsed = !p.sidebar_collapsed);
    let Some(app) = get() else { return };
    let width = app.window.width().max(app.window.default_width());
    apply_compact(width < NARROW_BELOW || prefs::get().sidebar_collapsed);
}
