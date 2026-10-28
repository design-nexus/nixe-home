//! Rooms: every device, grouped by Home Assistant area, with search and an
//! area filter. Tiles are kept across updates and only rebuilt when the
//! devices themselves change.
use crate::{
    app::{self, Home, Link},
    overview, prefs, present,
    tiles::{self, Tile},
};
use gtk::prelude::*;
use nyx_ui::widgets;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap},
};

const OTHER: &str = "Other";

pub struct Page {
    pub root: gtk::Widget,
    pub actions: gtk::Box,
    stack: gtk::Stack,
    empty: gtk::Box,
    chips: gtk::FlowBox,
    groups: gtk::Box,
    no_match: gtk::Box,
    show_all: gtk::ToggleButton,
    tiles: RefCell<HashMap<String, Tile>>,
    /// Area name per group, with its heading and grids.
    sections: RefCell<Vec<(String, gtk::Label, Vec<gtk::FlowBox>)>>,
    area: RefCell<Option<String>>,
    query: RefCell<String>,
    shown_link: RefCell<Option<Link>>,
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
        let groups = widgets::vbox(0);
        body.append(&groups);
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
        let show_all = gtk::ToggleButton::new();
        show_all.set_icon_name("view-reveal-symbolic");
        show_all.add_css_class("bar-button");
        show_all.set_valign(gtk::Align::Center);
        show_all.set_tooltip_text(Some("Show diagnostic and hidden entities"));
        show_all.update_property(&[gtk::accessible::Property::Label(
            "Show diagnostic and hidden entities",
        )]);
        show_all.set_active(prefs::get().show_all_entities);
        show_all.connect_toggled(|button| {
            let on = button.is_active();
            if prefs::get().show_all_entities != on {
                prefs::update(|p| p.show_all_entities = on);
                app::rebuild();
            }
        });
        actions.append(&show_all);
        Self {
            root: stack.clone().upcast(),
            actions,
            stack,
            empty,
            chips,
            groups,
            no_match,
            show_all,
            tiles: RefCell::new(HashMap::new()),
            sections: RefCell::new(Vec::new()),
            area: RefCell::new(None),
            query: RefCell::new(String::new()),
            shown_link: RefCell::new(None),
        }
    }

    pub fn set_query(&self, text: &str, home: &Home) {
        *self.query.borrow_mut() = text.trim().to_lowercase();
        self.filter(home);
    }

    pub fn refresh(&self, home: &Home, changed: Option<&BTreeSet<String>>) {
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
        let show_all = prefs::get().show_all_entities;
        if self.show_all.is_active() != show_all {
            self.show_all.set_active(show_all);
        }
        if let Some(ids) = changed {
            let tiles = self.tiles.borrow();
            let in_place = ids
                .iter()
                .all(|id| match (home.entities.get(id), tiles.get(id)) {
                    (Some(entity), Some(tile)) => {
                        let same = tile.signature == present::signature(entity);
                        if same {
                            tile.update(entity);
                        }
                        same
                    }
                    (Some(entity), None) => !present::listed(entity, show_all),
                    (None, _) => false,
                });
            drop(tiles);
            if in_place {
                self.filter(home);
                return;
            }
        }
        self.build(home, show_all);
    }

    fn build(&self, home: &Home, show_all: bool) {
        let mut groups: BTreeMap<String, Vec<&crate::ha::Entity>> = BTreeMap::new();
        for entity in home
            .entities
            .values()
            .filter(|e| present::listed(e, show_all))
        {
            groups
                .entry(entity.area_name.clone().unwrap_or_else(|| OTHER.into()))
                .or_default()
                .push(entity);
        }
        let mut order: Vec<String> = home
            .areas
            .iter()
            .map(|a| a.name.clone())
            .filter(|name| groups.contains_key(name))
            .collect();
        for name in groups.keys() {
            if name != OTHER && !order.contains(name) {
                order.push(name.clone());
            }
        }
        if groups.contains_key(OTHER) {
            order.push(OTHER.into());
        }
        if let Some(area) = self.area.borrow().clone()
            && !order.contains(&area)
        {
            *self.area.borrow_mut() = None;
        }

        while let Some(child) = self.chips.first_child() {
            self.chips.remove(&child);
        }
        let first = gtk::ToggleButton::with_label("All");
        first.add_css_class("chip");
        for name in std::iter::once(None).chain(order.iter().map(Some)) {
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
                    app.rooms.filter(&app.home.borrow());
                }
            });
            self.chips.insert(&chip, -1);
        }
        let mut child = self.chips.first_child();
        while let Some(c) = child {
            c.set_focusable(false);
            child = c.next_sibling();
        }

        let mut old = std::mem::take(&mut *self.tiles.borrow_mut());
        let mut tiles = HashMap::new();
        while let Some(child) = self.groups.first_child() {
            self.groups.remove(&child);
        }
        let mut sections = Vec::new();
        for name in order {
            let mut entities = groups.remove(&name).unwrap_or_default();
            entities.sort_by_key(|e| {
                (
                    matches!(present::primary(e), present::Primary::None) as u8,
                    e.name().to_lowercase(),
                )
            });
            let heading = widgets::label("", "group-title");
            self.groups.append(&heading);
            let (rich, plain): (Vec<_>, Vec<_>) =
                entities.into_iter().partition(|e| tiles::is_rich(e));
            let mut flows = Vec::new();
            for part in [rich, plain] {
                if part.is_empty() {
                    continue;
                }
                let flow = tiles::grid();
                flow.update_property(&[gtk::accessible::Property::Label(&name)]);
                for entity in part {
                    let tile = match old.remove(&entity.entity_id) {
                        Some(tile) if tile.signature == present::signature(entity) => {
                            if let Some(parent) = tile.child.parent().and_downcast::<gtk::FlowBox>()
                            {
                                parent.remove(&tile.child);
                            }
                            tile.update(entity);
                            tile
                        }
                        _ => Tile::new(entity),
                    };
                    flow.insert(&tile.child, -1);
                    tiles.insert(entity.entity_id.clone(), tile);
                }
                flow.connect_child_activated(|_, child| {
                    if let Some(app) = app::get()
                        && let Some(tile) =
                            app.rooms.tiles.borrow().get(child.widget_name().as_str())
                    {
                        tile.activate();
                    }
                });
                if !flows.is_empty() {
                    flow.set_margin_top(12);
                }
                self.groups.append(&flow);
                flows.push(flow);
            }
            sections.push((name, heading, flows));
        }
        *self.tiles.borrow_mut() = tiles;
        *self.sections.borrow_mut() = sections;
        self.filter(home);
    }

    /// Show the tiles that match the area chip and the search.
    fn filter(&self, home: &Home) {
        let query = self.query.borrow().clone();
        let area = self.area.borrow().clone();
        let mut shown = 0;
        for (name, heading, flows) in self.sections.borrow().iter() {
            let in_area = area.as_ref().is_none_or(|a| a == name);
            let mut count = 0;
            let mut first_shown = true;
            for flow in flows {
                let mut in_flow = 0;
                let mut child = flow.first_child();
                while let Some(c) = child {
                    child = c.next_sibling();
                    let Some(c) = c.downcast_ref::<gtk::FlowBoxChild>() else {
                        continue;
                    };
                    let id = c.widget_name();
                    let matches = in_area
                        && (query.is_empty()
                            || home.entities.get(id.as_str()).is_some_and(|e| {
                                [
                                    e.name(),
                                    e.entity_id.clone(),
                                    e.device_name.clone().unwrap_or_default(),
                                    name.clone(),
                                ]
                                .iter()
                                .any(|text| text.to_lowercase().contains(&query))
                            }));
                    c.set_visible(matches);
                    in_flow += usize::from(matches);
                }
                flow.set_visible(in_flow > 0);
                flow.set_margin_top(if in_flow > 0 && !first_shown { 12 } else { 0 });
                first_shown &= in_flow == 0;
                count += in_flow;
            }
            heading.set_text(&format!("{} · {count}", name.to_uppercase()));
            heading.set_visible(count > 0);
            shown += count;
        }
        while let Some(child) = self.no_match.first_child() {
            self.no_match.remove(&child);
        }
        self.no_match.set_visible(shown == 0);
        if shown == 0 {
            let (title, text) = if query.is_empty() {
                (
                    "No devices here",
                    "Turn on Show diagnostic and hidden entities to see everything Home Assistant has.".to_string(),
                )
            } else {
                (
                    "No devices match",
                    format!(
                        "Nothing matches “{}”. Search looks at names, devices and rooms.",
                        gtk::glib::markup_escape_text(&query)
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
