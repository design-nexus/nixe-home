use crate::{
    credentials, desktop_ui,
    ha::{self, Entity, Update},
    prefs, theme,
};
use gtk::prelude::*;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    Overview,
    Home,
    System,
    Automations,
    Activity,
    Integrations,
}

impl Section {
    const ALL: [Self; 6] = [
        Self::Overview,
        Self::Home,
        Self::System,
        Self::Automations,
        Self::Activity,
        Self::Integrations,
    ];
    fn id(self) -> &'static str {
        match self {
            Self::Overview => "overview",
            Self::Home => "home",
            Self::System => "system",
            Self::Automations => "automations",
            Self::Activity => "activity",
            Self::Integrations => "integrations",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Home => "Home",
            Self::System => "System",
            Self::Automations => "Automations",
            Self::Activity => "Activity",
            Self::Integrations => "Integrations",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Overview => "view-grid-symbolic",
            Self::Home => "go-home-symbolic",
            Self::System => "computer-symbolic",
            Self::Automations => "applications-engineering-symbolic",
            Self::Activity => "document-open-recent-symbolic",
            Self::Integrations => "network-server-symbolic",
        }
    }
    fn parse(id: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|s| s.id() == id)
            .unwrap_or(Self::Overview)
    }
}

struct Ui {
    window: gtk::ApplicationWindow,
    stack: gtk::Stack,
    nav: Vec<(Section, gtk::Button)>,
    crumb: gtk::Label,
    status: gtk::Label,
    entities: gtk::Box,
    overview: gtk::Box,
    desktop_pages: BTreeMap<&'static str, gtk::Box>,
    states: BTreeMap<String, Entity>,
    area_count: usize,
    device_count: usize,
    base: String,
    token: String,
    cancel: Option<async_channel::Sender<()>>,
}

thread_local! { static UI: RefCell<Option<Rc<RefCell<Ui>>>> = const { RefCell::new(None) }; }

pub fn present(app: &gtk::Application) {
    if let Some(ui) = UI.with(|v| v.borrow().clone()) {
        ui.borrow().window.present();
        return;
    }
    theme::install();
    let prefs = prefs::get();
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Home")
        .default_width(1120)
        .default_height(780)
        .build();
    window.add_css_class("home-window");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    window.set_icon_name(Some("io.github.design_nexus.nixe.Home"));
    nixe_ui::keys::install(&window, nixe_ui::keys::Common::default());

    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    top.set_height_request(46);
    top.set_margin_start(14);
    top.set_margin_end(10);
    let crumb = gtk::Label::new(Some("Home / Overview"));
    crumb.set_halign(gtk::Align::Start);
    crumb.set_hexpand(true);
    let connect = gtk::Button::with_label("Connect Home Assistant");
    connect.add_css_class("flat");
    let settings = gtk::Button::from_icon_name("emblem-system-symbolic");
    settings.set_tooltip_text(Some("Connection settings"));
    top.append(&crumb);
    top.append(&connect);
    top.append(&settings);
    frame.append(&top);

    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let sidebar = gtk::Box::new(gtk::Orientation::Vertical, 8);
    sidebar.add_css_class("home-sidebar");
    sidebar.set_hexpand(false);
    sidebar.set_width_request(210);
    let brand = gtk::Label::new(Some("Home"));
    brand.set_halign(gtk::Align::Start);
    brand.set_margin_top(16);
    brand.set_margin_start(18);
    brand.add_css_class("home-heading");
    sidebar.append(&brand);
    let nav_list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    nav_list.set_hexpand(false);
    let mut nav = Vec::new();
    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(if prefs.reduce_motion { 0 } else { 150 });
    let entities = gtk::Box::new(gtk::Orientation::Vertical, 8);
    entities.add_css_class("home-content");
    entities.set_hexpand(true);
    entities.set_vexpand(true);
    let overview = gtk::Box::new(gtk::Orientation::Vertical, 12);
    overview.add_css_class("home-content");
    overview.set_hexpand(true);
    overview.set_vexpand(true);
    let mut desktop_pages = BTreeMap::new();
    for section in Section::ALL {
        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.add_css_class("home-nav");
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        row.append(&gtk::Image::from_icon_name(section.icon()));
        let label = gtk::Label::new(Some(section.title()));
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(true);
        row.append(&label);
        button.set_child(Some(&row));
        nav_list.append(&button);
        let page = match section {
            Section::Overview => overview.clone().upcast::<gtk::Widget>(),
            Section::Home => entities.clone().upcast::<gtk::Widget>(),
            Section::System | Section::Automations | Section::Activity => {
                let page = desktop_ui::page(section.id());
                desktop_pages.insert(section.id(), page.clone());
                page.upcast::<gtk::Widget>()
            }
            _ => readiness_page(section).upcast::<gtk::Widget>(),
        };
        stack.add_named(&page, Some(section.id()));
        nav.push((section, button));
    }
    let nav_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .vexpand(true)
        .child(&nav_list)
        .build();
    sidebar.append(&nav_scroll);
    let sidebar_bottom = gtk::Label::new(Some("Home Assistant · Desktop automation"));
    sidebar_bottom.set_halign(gtk::Align::Start);
    sidebar_bottom.set_margin_start(16);
    sidebar_bottom.set_margin_bottom(12);
    sidebar_bottom.add_css_class("home-muted");
    sidebar.append(&sidebar_bottom);
    body.append(&sidebar);
    let content = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(&stack)
        .build();
    content.set_hexpand(true);
    content.set_vexpand(true);
    body.append(&content);
    frame.append(&body);
    let status = gtk::Label::new(Some("Connect to Home Assistant to discover your home."));
    status.set_halign(gtk::Align::Start);
    status.set_height_request(32);
    status.set_margin_start(14);
    status.add_css_class("home-muted");
    frame.append(&status);
    window.set_child(Some(&frame));

    let ui = Rc::new(RefCell::new(Ui {
        window: window.clone(),
        stack,
        nav,
        crumb,
        status,
        entities,
        overview,
        desktop_pages,
        states: BTreeMap::new(),
        area_count: 0,
        device_count: 0,
        base: prefs.home_url.clone(),
        token: String::new(),
        cancel: None,
    }));
    UI.with(|slot| *slot.borrow_mut() = Some(ui.clone()));
    {
        let u = ui.borrow_mut();
        let message = gtk::Label::new(Some("Connect Home Assistant to see your home at a glance."));
        message.set_halign(gtk::Align::Start);
        message.add_css_class("home-muted");
        u.overview.append(&message);
        let button = gtk::Button::with_label("Connect Home Assistant");
        button.set_halign(gtk::Align::Start);
        let ui2 = ui.clone();
        button.connect_clicked(move |_| connection_dialog(&ui2));
        u.overview.append(&button);
    }
    for (section, button) in ui.borrow().nav.clone() {
        let ui2 = ui.clone();
        button.connect_clicked(move |_| show_section(&ui2, section));
    }
    let ui2 = ui.clone();
    connect.connect_clicked(move |_| connection_dialog(&ui2));
    let ui2 = ui.clone();
    settings.connect_clicked(move |_| connection_dialog(&ui2));
    show_section(&ui, Section::parse(&prefs.last_section));
    if !prefs.home_url.is_empty() {
        if let Ok(token) = credentials::load() {
            start_connection(&ui, prefs.home_url, token);
        } else {
            ui.borrow()
                .status
                .set_text("Reconnect to Home Assistant to restore access.");
        }
    }
    window.present();
}

fn readiness_page(section: Section) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.set_valign(gtk::Align::Start);
    page.add_css_class("home-content");
    let heading = gtk::Label::new(Some(section.title()));
    heading.set_halign(gtk::Align::Start);
    heading.add_css_class("home-heading");
    page.append(&heading);
    let text = match section {
        Section::System => {
            "Desktop status and actions will be managed here. The local provider is not connected yet."
        }
        Section::Automations => {
            "The unified automation editor is planned after the Home Assistant frontend foundation."
        }
        Section::Activity => {
            "Automation runs will appear here when the automation engine is available."
        }
        Section::Integrations => {
            "Home Assistant connection settings are available from the top bar. Local desktop providers will be added in a later phase."
        }
        _ => "",
    };
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("home-card");
    let description = gtk::Label::new(Some(text));
    description.set_halign(gtk::Align::Start);
    description.set_wrap(true);
    description.add_css_class("home-muted");
    card.append(&description);
    page.append(&card);
    page
}

fn show_section(ui: &Rc<RefCell<Ui>>, section: Section) {
    let borrowed = ui.borrow();
    borrowed.stack.set_visible_child_name(section.id());
    borrowed
        .crumb
        .set_text(&format!("Home / {}", section.title()));
    for (candidate, button) in &borrowed.nav {
        if *candidate == section {
            button.add_css_class("active");
        } else {
            button.remove_css_class("active");
        }
    }
    drop(borrowed);
    if let Some(page) = ui.borrow().desktop_pages.get(section.id()) {
        let borrowed = ui.borrow();
        desktop_ui::refresh(
            page.clone(),
            section.id(),
            crate::builder::HomeContext {
                base: borrowed.base.clone(),
                token: borrowed.token.clone(),
                entities: borrowed
                    .states
                    .values()
                    .map(|e| (e.entity_id.clone(), e.name(), e.state.clone()))
                    .collect(),
                automations: borrowed
                    .states
                    .values()
                    .filter(|e| e.domain() == "automation")
                    .filter_map(|e| {
                        e.attributes["id"]
                            .as_str()
                            .map(|id| (id.to_string(), e.state.clone()))
                    })
                    .collect(),
            },
        );
    }
    prefs::update(|p| p.last_section = section.id().into());
}

#[allow(deprecated)]
fn connection_dialog(ui: &Rc<RefCell<Ui>>) {
    let parent = ui.borrow().window.clone();
    let dialog = gtk::Dialog::with_buttons(
        Some("Connect to Home Assistant"),
        Some(&parent),
        gtk::DialogFlags::MODAL,
        &[
            ("Disconnect", gtk::ResponseType::Reject),
            ("Cancel", gtk::ResponseType::Cancel),
            ("Connect", gtk::ResponseType::Accept),
        ],
    );
    dialog.set_default_size(460, 220);
    let content = dialog.content_area();
    content.set_spacing(10);
    content.set_margin_top(14);
    content.set_margin_bottom(14);
    content.set_margin_start(16);
    content.set_margin_end(16);
    let intro = gtk::Label::new(Some(
        "Enter your Home Assistant URL and a long-lived access token. The token is stored in your desktop keyring.",
    ));
    intro.set_wrap(true);
    intro.set_halign(gtk::Align::Start);
    content.append(&intro);
    let url = gtk::Entry::new();
    url.set_placeholder_text(Some("http://homeassistant.local:8123"));
    url.set_text(&ui.borrow().base);
    content.append(&url);
    let token = gtk::Entry::new();
    token.set_visibility(false);
    token.set_placeholder_text(Some("Long-lived access token"));
    content.append(&token);
    let error = gtk::Label::new(None);
    error.set_halign(gtk::Align::Start);
    error.add_css_class("home-error");
    content.append(&error);
    let ui2 = ui.clone();
    dialog.connect_response(move |dialog, response| {
        if response == gtk::ResponseType::Reject {
            let _ = credentials::delete();
            prefs::update(|p| p.home_url.clear());
            if let Ok(mut state) = ui2.try_borrow_mut() {
                state.base.clear();
                state.token.clear();
                if let Some(cancel) = state.cancel.take() {
                    let _ = cancel.try_send(());
                }
                state.states.clear();
                render_entities(&mut state);
                state.status.set_text("Disconnected from Home Assistant.");
            }
            dialog.close();
            return;
        }
        if response != gtk::ResponseType::Accept {
            dialog.close();
            return;
        }
        let base = url.text().trim().trim_end_matches('/').to_string();
        let secret = token.text().to_string();
        if !(base.starts_with("http://") || base.starts_with("https://")) || secret.is_empty() {
            error.set_text("Enter an http:// or https:// URL and a token.");
            return;
        }
        if let Err(message) = credentials::store(&secret) {
            error.set_text(&message);
            return;
        }
        prefs::update(|p| p.home_url = base.clone());
        start_connection(&ui2, base, secret);
        dialog.close();
    });
    dialog.present();
}

fn start_connection(ui: &Rc<RefCell<Ui>>, base: String, token: String) {
    let (cancel_tx, cancel_rx) = async_channel::bounded(1);
    {
        let mut u = ui.borrow_mut();
        if let Some(cancel) = u.cancel.take() {
            let _ = cancel.try_send(());
        }
        u.base = base.clone();
        u.token = token.clone();
        u.cancel = Some(cancel_tx);
        u.status.set_text("Connecting and loading devices…");
    }
    let (sender, receiver) = async_channel::unbounded::<Update>();
    let base_for_worker = base.clone();
    let token_for_worker = token.clone();
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        runtime.block_on(async move {
            let result = tokio::select! {
                _ = cancel_rx.recv() => return,
                result = ha::fetch_inventory(&base_for_worker, &token_for_worker) => result,
            };
            match result {
                Ok(inventory) => {
                    let metadata = inventory.metadata.clone();
                    let _ = sender.send(Update::Connected(inventory)).await;
                    ha::start_live_updates(
                        base_for_worker,
                        token_for_worker,
                        sender.clone(),
                        cancel_rx.clone(),
                        metadata,
                    );
                }
                Err(error) => {
                    let _ = sender.send(Update::Status(error)).await;
                }
            }
        });
    });
    let ui = ui.clone();
    gtk::glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
        loop {
            match receiver.try_recv() {
                Ok(Update::Connected(inventory)) => {
                    let mut u = ui.borrow_mut();
                    u.area_count = inventory.area_count;
                    u.device_count = inventory.device_count;
                    u.states = inventory
                        .entities
                        .into_iter()
                        .map(|e| (e.entity_id.clone(), e))
                        .collect();
                    render_entities(&mut u);
                    u.status.set_text(&format!(
                        "Connected · {} areas · {} devices · live updates enabled",
                        u.area_count, u.device_count
                    ));
                }
                Ok(Update::Entity(entity)) => {
                    let mut u = ui.borrow_mut();
                    u.states.insert(entity.entity_id.clone(), entity);
                    render_entities(&mut u);
                }
                Ok(Update::Removed(entity_id)) => {
                    let mut u = ui.borrow_mut();
                    u.states.remove(&entity_id);
                    render_entities(&mut u);
                }
                Ok(Update::Status(text)) => ui.borrow().status.set_text(&text),
                Err(async_channel::TryRecvError::Empty) => break,
                Err(async_channel::TryRecvError::Closed) => return gtk::glib::ControlFlow::Break,
            }
        }
        gtk::glib::ControlFlow::Continue
    });
}

fn render_entities(ui: &mut Ui) {
    clear_box(&ui.entities);
    clear_box(&ui.overview);
    let home_heading = gtk::Label::new(Some("Devices and entities"));
    home_heading.set_halign(gtk::Align::Start);
    home_heading.add_css_class("home-heading");
    ui.entities.append(&home_heading);
    let total = ui.states.len();
    let lights = ui.states.values().filter(|e| e.domain() == "light").count();
    let online = ui
        .states
        .values()
        .filter(|e| !matches!(e.state.as_str(), "unavailable" | "unknown"))
        .count();
    for (title, value) in [
        ("Entities", total.to_string()),
        ("Lights", lights.to_string()),
        ("Areas", ui.area_count.to_string()),
        ("Devices", ui.device_count.to_string()),
        ("Available", online.to_string()),
    ] {
        let card = metric_card(title, &value);
        ui.overview.append(&card);
    }
    let h = gtk::Label::new(Some("Favorite devices"));
    h.set_halign(gtk::Align::Start);
    h.add_css_class("home-heading");
    ui.overview.append(&h);
    let favorites = prefs::get().favorite_entities;
    let favs: Vec<_> = ui
        .states
        .values()
        .filter(|e| favorites.contains(&e.entity_id))
        .cloned()
        .collect();
    if favs.is_empty() {
        let note = gtk::Label::new(Some("Star devices on Home to pin them here."));
        note.set_wrap(true);
        note.set_halign(gtk::Align::Start);
        note.add_css_class("home-muted");
        ui.overview.append(&note);
    } else {
        for entity in favs {
            ui.overview
                .append(&entity_row(&entity, &ui.base, &ui.token, &ui.window));
        }
    }
    let note = gtk::Label::new(Some("Live state changes arrive from Home Assistant."));
    note.set_halign(gtk::Align::Start);
    note.add_css_class("home-muted");
    ui.overview.append(&note);
    let mut grouped: BTreeMap<String, Vec<&Entity>> = BTreeMap::new();
    for entity in ui.states.values() {
        let name = entity
            .area_name
            .clone()
            .unwrap_or_else(|| format!("Other · {}", domain_label(entity.domain())));
        grouped.entry(name).or_default().push(entity);
    }
    for (area, entities) in grouped {
        let group = gtk::Label::new(Some(&format!("{} · {}", area, entities.len())));
        group.set_halign(gtk::Align::Start);
        group.add_css_class("home-muted");
        group.set_margin_top(8);
        ui.entities.append(&group);
        for entity in entities {
            ui.entities
                .append(&entity_row(entity, &ui.base, &ui.token, &ui.window));
        }
    }
}

fn metric_card(label: &str, value: &str) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 6);
    card.add_css_class("home-card");
    let name = gtk::Label::new(Some(label));
    name.set_halign(gtk::Align::Start);
    name.add_css_class("home-muted");
    let number = gtk::Label::new(Some(value));
    number.set_halign(gtk::Align::Start);
    number.add_css_class("home-heading");
    card.append(&name);
    card.append(&number);
    card.upcast()
}

fn entity_row(
    entity: &Entity,
    base: &str,
    token: &str,
    parent: &gtk::ApplicationWindow,
) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("home-card");
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.set_hexpand(true);
    let name = gtk::Label::new(Some(&entity.name()));
    name.set_halign(gtk::Align::Start);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let device = entity
        .device_name
        .as_deref()
        .map(|name| format!("{name} · "))
        .unwrap_or_default();
    let sub = gtk::Label::new(Some(&format!(
        "{device}{} · {}",
        entity.entity_id, entity.state
    )));
    sub.set_halign(gtk::Align::Start);
    sub.add_css_class("home-muted");
    sub.add_css_class("technical");
    text.append(&name);
    text.append(&sub);
    row.append(&text);
    let favorite_ids = prefs::get().favorite_entities;
    let favorite = favorite_ids.iter().any(|id| id == &entity.entity_id);
    let star = gtk::Button::with_label(if favorite { "★" } else { "☆" });
    star.set_tooltip_text(Some(if favorite {
        "Remove from Overview"
    } else {
        "Add to Overview"
    }));
    let entity_id = entity.entity_id.clone();
    star.connect_clicked(move |_| {
        prefs::update(|p| {
            if p.favorite_entities.iter().any(|id| id == &entity_id) {
                p.favorite_entities.retain(|id| id != &entity_id);
            } else {
                p.favorite_entities.push(entity_id.clone());
            }
        });
        UI.with(|slot| {
            if let Some(ui) = slot.borrow().as_ref()
                && let Ok(mut ui) = ui.try_borrow_mut()
            {
                render_entities(&mut ui);
            }
        });
    });
    row.append(&star);
    if supports_quick_control(entity) {
        let action = if entity.domain() == "lock" {
            if entity.state == "locked" {
                "Unlock"
            } else {
                "Lock"
            }
        } else if entity.state == "on" {
            "Turn off"
        } else {
            "Turn on"
        };
        let button = gtk::Button::with_label(action);
        button.add_css_class("flat");
        let e = entity.clone();
        let base = base.to_string();
        let token = token.to_string();
        let parent = parent.clone();
        button.connect_clicked(move |_| {
            let e = e.clone();
            let base = base.clone();
            let token = token.clone();
            if e.domain() == "lock" {
                let operation = if e.state == "locked" {
                    "Unlock"
                } else {
                    "Lock"
                };
                let alert = gtk::AlertDialog::builder()
                    .message(format!("{operation} {}?", e.name()))
                    .detail("Home Assistant will send this command to the lock.")
                    .buttons(["Cancel", operation])
                    .cancel_button(0)
                    .default_button(0)
                    .build();
                alert.choose(
                    Some(&parent),
                    None::<&gtk::gio::Cancellable>,
                    move |result| {
                        if result == Ok(1) {
                            call_home_service(base, token, e);
                        }
                    },
                );
            } else {
                call_home_service(base, token, e);
            }
        });
        row.append(&button);
    }
    row.upcast()
}

fn supports_quick_control(e: &Entity) -> bool {
    matches!(
        e.domain(),
        "light" | "switch" | "input_boolean" | "fan" | "lock"
    )
}

fn call_home_service(base: String, token: String, entity: Entity) {
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        let result = runtime.block_on(ha::call_service(&base, &token, &entity));
        if let Err(message) = result {
            gtk::glib::MainContext::default().invoke(move || {
                UI.with(|slot| {
                    if let Some(ui) = slot.borrow().as_ref() {
                        ui.borrow().status.set_text(&message);
                    }
                })
            });
        }
    });
}
fn domain_label(domain: &str) -> &str {
    match domain {
        "light" => "Lights",
        "switch" => "Switches",
        "sensor" => "Sensors",
        "binary_sensor" => "Sensors",
        "climate" => "Climate",
        "camera" => "Cameras",
        "vacuum" => "Vacuums",
        "cover" => "Covers",
        _ => domain,
    }
}
fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}
