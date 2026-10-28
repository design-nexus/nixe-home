//! Rooms: one card per device, in sections by type (Lights, Climate, …) laid
//! out on the card grid, with the room's scenes as colored tiles. The room
//! chips pick a room; All shows every room. Cards keep themselves current;
//! the layout is rebuilt only when devices come or go or another room is
//! chosen. Arrange mode moves and resizes cards and sections.
use crate::{
    app::{self, Home, Link},
    cards,
    device::{self, Device, Kind},
    grid::{self, CardGrid, Sections},
    light_groups, overview, prefs,
};
use gtk::{gdk, glib, prelude::*};
use nyx_ui::widgets;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};

const OTHER: &str = "Other";
const SCENES: &str = "scenes";

pub struct Page {
    pub root: gtk::Widget,
    pub actions: gtk::Box,
    stack: gtk::Stack,
    empty: gtk::Box,
    chips: gtk::FlowBox,
    flow: Sections,
    no_match: gtk::Box,
    show_all: gtk::ToggleButton,
    arrange: gtk::ToggleButton,
    reset: gtk::Button,
    arranging: Cell<bool>,
    devices: RefCell<Vec<Device>>,
    /// Each section's key and heading, to count what search leaves.
    headings: RefCell<Vec<(String, String, gtk::Label)>>,
    area: RefCell<Option<String>>,
    query: RefCell<String>,
    shown_link: RefCell<Option<Link>>,
}

/// The Arrange button and its Reset, shared by Rooms and Cameras.
pub fn arrange_buttons(actions: &gtk::Box) -> (gtk::ToggleButton, gtk::Button) {
    let reset = gtk::Button::with_label("Reset");
    reset.set_valign(gtk::Align::Center);
    reset.set_tooltip_text(Some(
        "Put this page's cards back in their first order and sizes",
    ));
    reset.set_visible(false);
    actions.append(&reset);
    let arrange = gtk::ToggleButton::new();
    arrange.set_icon_name("view-app-grid-symbolic");
    arrange.add_css_class("bar-button");
    arrange.set_valign(gtk::Align::Center);
    arrange.set_tooltip_text(Some("Arrange cards: drag them, change their size"));
    arrange.update_property(&[gtk::accessible::Property::Label("Arrange cards")]);
    actions.append(&arrange);
    (arrange, reset)
}

/// Esc leaves Arrange mode.
pub fn escape_leaves(root: &impl IsA<gtk::Widget>, arrange: &gtk::ToggleButton) {
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let arrange = arrange.clone();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape && arrange.is_active() {
            arrange.set_active(false);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    root.add_controller(keys);
}

impl Page {
    pub fn new() -> Self {
        let stack = gtk::Stack::new();
        let empty = widgets::vbox(0);
        empty.set_vexpand(true);
        stack.add_named(&empty, Some("empty"));
        let body = widgets::vbox(0);
        body.add_css_class("settings-page");
        body.add_css_class("home-page");
        let chips = gtk::FlowBox::new();
        chips.add_css_class("area-chips");
        chips.set_selection_mode(gtk::SelectionMode::None);
        chips.set_column_spacing(6);
        chips.set_row_spacing(6);
        chips.set_max_children_per_line(30);
        chips.set_valign(gtk::Align::Start);
        chips.update_property(&[gtk::accessible::Property::Label("Rooms")]);
        body.append(&chips);
        let flow = Sections::new("rooms");
        body.append(&flow);
        let no_match = widgets::vbox(0);
        no_match.set_visible(false);
        body.append(&no_match);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .child(&body)
            .build();
        stack.add_named(&scroll, Some("content"));
        let actions = widgets::hbox(2);
        let (arrange, reset) = arrange_buttons(&actions);
        let show_all = gtk::ToggleButton::new();
        show_all.set_icon_name("view-reveal-symbolic");
        show_all.add_css_class("bar-button");
        show_all.set_valign(gtk::Align::Center);
        show_all.set_tooltip_text(Some(
            "Also show hidden devices and ones with only diagnostic entities",
        ));
        show_all.update_property(&[gtk::accessible::Property::Label("Show every device")]);
        show_all.set_active(prefs::get().show_all_entities);
        show_all.connect_toggled(|button| {
            let on = button.is_active();
            if prefs::get().show_all_entities != on {
                prefs::update(|p| p.show_all_entities = on);
                app::rebuild();
            }
        });
        actions.append(&show_all);
        arrange.connect_toggled(|b| {
            if let Some(app) = app::get() {
                app.rooms.set_arranging(b.is_active());
            }
        });
        reset.connect_clicked(|_| {
            if let Some(app) = app::get() {
                let ids: Vec<String> = app
                    .rooms
                    .flow
                    .grids()
                    .iter()
                    .flat_map(|g| g.ids())
                    .collect();
                grid::reset("rooms", &ids);
                app::rebuild();
            }
        });
        escape_leaves(&stack, &arrange);
        Self {
            root: stack.clone().upcast(),
            actions,
            stack,
            empty,
            chips,
            flow,
            no_match,
            show_all,
            arrange,
            reset,
            arranging: Cell::new(false),
            devices: RefCell::new(Vec::new()),
            headings: RefCell::new(Vec::new()),
            area: RefCell::new(None),
            query: RefCell::new(String::new()),
            shown_link: RefCell::new(None),
        }
    }

    /// Developer aid: open on one room's tab.
    pub fn show_room(&self, room: &str, home: &Home) {
        *self.area.borrow_mut() = Some(room.to_string());
        self.build(home, prefs::get().show_all_entities);
    }

    /// Developer aid: open in Arrange mode.
    pub fn start_arranging(&self) {
        self.arrange.set_active(true);
    }

    fn set_arranging(&self, on: bool) {
        if self.arranging.replace(on) == on {
            return;
        }
        self.flow.set_arranging(on);
        self.reset.set_visible(on);
        if self.arrange.is_active() != on {
            self.arrange.set_active(on);
        }
        if !on {
            // Sizes are shared with other pages.
            app::rebuild();
        }
    }

    pub fn set_query(&self, text: &str, home: &Home) {
        *self.query.borrow_mut() = text.trim().to_lowercase();
        self.filter(home);
    }

    /// Rebuild the layout. Live changes reach the cards on their own.
    pub fn refresh(&self, home: &Home, changed: bool) {
        if !home.loaded() {
            if self.shown_link.borrow().as_ref() != Some(&home.link) {
                overview::set_empty(&self.empty, &home.link);
                *self.shown_link.borrow_mut() = Some(home.link.clone());
            }
            self.stack.set_visible_child_name("empty");
            return;
        }
        *self.shown_link.borrow_mut() = None;
        self.stack.set_visible_child_name("content");
        if changed {
            return;
        }
        let show_all = prefs::get().show_all_entities;
        if self.show_all.is_active() != show_all {
            self.show_all.set_active(show_all);
        }
        self.build(home, show_all);
    }

    /// The room chips, then the sections for the chosen room.
    fn build(&self, home: &Home, show_all: bool) {
        let devices = home.device_list(show_all);
        let scenes = device::scenes(&home.entities);
        let mut names: Vec<String> = home
            .areas
            .iter()
            .map(|a| a.name.clone())
            .filter(|name| {
                devices.iter().any(|d| d.area.as_ref() == Some(name)) || scenes.contains_key(name)
            })
            .collect();
        for d in &devices {
            if let Some(area) = &d.area
                && !names.contains(area)
            {
                names.push(area.clone());
            }
        }
        if devices.iter().any(|d| d.area.is_none()) || scenes.contains_key(OTHER) {
            names.push(OTHER.into());
        }
        if let Some(area) = self.area.borrow().clone()
            && !names.contains(&area)
        {
            *self.area.borrow_mut() = None;
        }

        while let Some(child) = self.chips.first_child() {
            self.chips.remove(&child);
        }
        let first = gtk::ToggleButton::with_label("All");
        first.add_css_class("chip");
        for name in std::iter::once(None).chain(names.iter().map(Some)) {
            let chip = match name {
                None => first.clone(),
                Some(name) => {
                    let chip = gtk::ToggleButton::with_label(name);
                    chip.add_css_class("chip");
                    chip.set_group(Some(&first));
                    chip
                }
            };
            chip.set_active(self.area.borrow().as_ref() == name);
            let area = name.cloned();
            chip.connect_toggled(move |chip| {
                if !chip.is_active() {
                    return;
                }
                if let Some(app) = app::get() {
                    *app.rooms.area.borrow_mut() = area.clone();
                    app.rooms.fill(&app.home.borrow());
                }
            });
            self.chips.insert(&chip, -1);
        }
        let mut child = self.chips.first_child();
        while let Some(c) = child {
            c.set_focusable(false);
            child = c.next_sibling();
        }
        *self.devices.borrow_mut() = devices;
        self.fill(home);
    }

    /// The sections for the chosen room (every room for All).
    fn fill(&self, home: &Home) {
        let area = self.area.borrow().clone();
        let in_room = |a: Option<&String>| match &area {
            None => true,
            Some(want) if want == OTHER => a.is_none(),
            Some(want) => a == Some(want),
        };
        let devices = self.devices.borrow();
        let shown: Vec<&Device> = devices
            .iter()
            .filter(|d| in_room(d.area.as_ref()))
            .collect();
        let show_area = area.is_none();
        let order = prefs::get().card_order;
        let saved = |key: &str| order.get(key).cloned().unwrap_or_default();
        let maker = cards::maker(shown.iter().map(|d| (*d).clone()).collect(), show_area);

        self.flow.clear();
        // Each section's key, title, heading button and cards with their sizes.
        type Cards = Vec<(String, Vec<grid::Size>)>;
        let mut built: Vec<(String, String, Option<gtk::Widget>, Cards)> = Vec::new();

        let scenes = device::scenes(&home.entities);
        let p = prefs::get();
        let scene_ids: Vec<String> = match &area {
            None => scenes.values().flatten().cloned().collect(),
            Some(a) => scenes.get(a).cloned().unwrap_or_default(),
        }
        .into_iter()
        .filter(|id| p.show_all_entities || !p.hidden_devices.contains(&format!("entity:{id}")))
        .collect();
        if !scene_ids.is_empty() {
            let ids = grid::ordered(&saved(&format!("rooms:{SCENES}")), scene_ids);
            let list = ids
                .into_iter()
                .map(|id| (id, cards::SCENE_SIZES.to_vec()))
                .collect();
            built.push((SCENES.into(), "Scenes".into(), None, list));
        }

        let mut by_kind: BTreeMap<Kind, Vec<&Device>> = BTreeMap::new();
        for d in &shown {
            by_kind.entry(d.kind).or_default().push(d);
        }
        let room_sets: Vec<(String, Vec<String>)> = home
            .room_light_sets()
            .into_iter()
            .filter(|(room, _)| in_room(Some(room)))
            .collect();
        for (kind, mut list) in by_kind {
            let mut ids: Cards = Vec::new();
            if kind == Kind::Light {
                // Groups first: Home Assistant's, then each room's lights.
                list.sort_by_key(|d| d.members.is_none());
                let groups = list.iter().take_while(|d| d.members.is_some()).count();
                for d in &list[..groups] {
                    ids.push((d.id.clone(), cards::size_options(d, home)));
                }
                for (room, _) in &room_sets {
                    ids.push((
                        format!("{}{room}", cards::ROOM_LIGHTS),
                        cards::ROOM_LIGHT_SIZES.to_vec(),
                    ));
                }
                for d in &list[groups..] {
                    ids.push((d.id.clone(), cards::size_options(d, home)));
                }
            } else {
                for d in list {
                    ids.push((d.id.clone(), cards::size_options(d, home)));
                }
            }
            let options: BTreeMap<String, Vec<grid::Size>> = ids.iter().cloned().collect();
            let default: Vec<String> = ids.into_iter().map(|(id, _)| id).collect();
            let key = format!("rooms:{}", kind.key());
            let ids = grid::ordered(&saved(&key), default)
                .into_iter()
                .map(|id| {
                    let o = options[&id].clone();
                    (id, o)
                })
                .collect();
            let extra = (kind == Kind::Light && area.is_none()).then(|| {
                let new = widgets::bar_button("list-add-symbolic", "New light group");
                new.connect_clicked(|_| light_groups::create(None, Vec::new()));
                new.upcast::<gtk::Widget>()
            });
            built.push((kind.key().into(), kind.title().into(), extra, ids));
        }
        drop(devices);

        let keys = Sections::order("rooms", built.iter().map(|b| b.0.clone()).collect());
        let mut sorted = Vec::new();
        for key in keys {
            if let Some(i) = built.iter().position(|b| b.0 == key) {
                sorted.push(built.remove(i));
            }
        }
        let mut headings = Vec::new();
        match &area {
            // A room has too few devices to split by type: one grid, scenes
            // first, then the types in the order All has them.
            Some(room) => {
                let key = format!("rooms:room:{room}");
                let g = CardGrid::new(&key, maker.clone());
                let all: Cards = sorted.into_iter().flat_map(|b| b.3).collect();
                let options: BTreeMap<String, Vec<grid::Size>> = all.iter().cloned().collect();
                let default: Vec<String> = all.into_iter().map(|(id, _)| id).collect();
                for id in grid::ordered(&saved(&key), default) {
                    cards::add_to(&g, &id, options[&id].clone());
                }
                g.update_property(&[gtk::accessible::Property::Label(room)]);
                self.flow.add_plain(&key, &g);
            }
            None => {
                for (key, title, extra, list) in sorted {
                    let g = CardGrid::new(&format!("rooms:{key}"), maker.clone());
                    for (id, options) in list {
                        cards::add_to(&g, &id, options);
                    }
                    let heading = widgets::label("", "group-title");
                    g.update_property(&[gtk::accessible::Property::Label(&title)]);
                    self.flow.add(&key, &heading, extra.as_ref(), &g);
                    headings.push((key, title, heading));
                }
            }
        }
        *self.headings.borrow_mut() = headings;
        self.flow.set_arranging(self.arranging.get());
        self.filter(home);
    }

    /// Show the cards that match the search, and count them.
    fn filter(&self, home: &Home) {
        let query = self.query.borrow().clone();
        let devices = self.devices.borrow();
        let by_id: BTreeMap<&str, &Device> = devices.iter().map(|d| (d.id.as_str(), d)).collect();
        let headings = self.headings.borrow();
        let mut shown = 0;
        for (key, section, g) in self.flow.sections() {
            let mut count = 0;
            for id in g.ids() {
                let matches = query.is_empty()
                    || if let Some(room) = id.strip_prefix(cards::ROOM_LIGHTS) {
                        room.to_lowercase().contains(&query) || "lights".contains(&query)
                    } else if let Some(d) = by_id.get(id.as_str()) {
                        d.matches(&query, &home.entities)
                    } else {
                        home.entities
                            .get(&id)
                            .is_some_and(|e| e.name().to_lowercase().contains(&query))
                    };
                g.set_shown(&id, matches);
                count += usize::from(matches);
            }
            if let Some((_, title, heading)) = headings.iter().find(|h| h.0 == key) {
                heading.set_text(&format!("{} · {count}", title.to_uppercase()));
            }
            section.set_visible(count > 0);
            shown += count;
        }
        self.flow.queue_resize();
        while let Some(child) = self.no_match.first_child() {
            self.no_match.remove(&child);
        }
        self.no_match.set_visible(shown == 0);
        if shown == 0 {
            let (title, text) = if query.is_empty() {
                (
                    "No devices here",
                    "Turn on Show every device to include hidden ones and ones with only diagnostic entities."
                        .to_string(),
                )
            } else {
                (
                    "No devices match",
                    format!(
                        "Nothing matches “{}”. Search looks at device names, their entities and rooms.",
                        glib::markup_escape_text(&query)
                    ),
                )
            };
            self.no_match.append(&widgets::empty_state(
                "edit-find-symbolic",
                title,
                &text,
                None,
            ));
        }
    }
}
