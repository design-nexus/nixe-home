//! Cameras: every camera with its last picture, each card saying its room.
//! Nothing else. Arrange mode moves and resizes the cards.
use crate::{
    app::{self, Home, Link},
    cards,
    device::{Device, Kind},
    grid::{self, CardGrid, Sections},
    overview, prefs, rooms,
};
use gtk::prelude::*;
use nyx_ui::widgets;
use std::cell::{Cell, RefCell};

pub struct Page {
    pub root: gtk::Widget,
    pub actions: gtk::Box,
    stack: gtk::Stack,
    empty: gtk::Box,
    flow: Sections,
    arrange: gtk::ToggleButton,
    reset: gtk::Button,
    arranging: Cell<bool>,
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
        let flow = Sections::new("cameras");
        body.append(&flow);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .child(&body)
            .build();
        stack.add_named(&scroll, Some("content"));
        let actions = widgets::hbox(2);
        let (arrange, reset) = rooms::arrange_buttons(&actions);
        arrange.connect_toggled(|b| {
            if let Some(app) = app::get() {
                app.cameras.set_arranging(b.is_active());
            }
        });
        reset.connect_clicked(|_| {
            if let Some(app) = app::get() {
                let ids: Vec<String> = app
                    .cameras
                    .flow
                    .grids()
                    .iter()
                    .flat_map(|g| g.ids())
                    .collect();
                grid::reset("cameras", &ids);
                app::rebuild();
            }
        });
        rooms::escape_leaves(&stack, &arrange);
        Self {
            root: stack.clone().upcast(),
            actions,
            stack,
            empty,
            flow,
            arrange,
            reset,
            arranging: Cell::new(false),
            shown_link: RefCell::new(None),
        }
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
            app::rebuild();
        }
    }

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
        if changed {
            return;
        }
        self.flow.clear();
        let devices = home.device_list(false);
        let cameras: Vec<&Device> = devices.iter().filter(|d| d.kind == Kind::Camera).collect();
        if cameras.is_empty() {
            while let Some(child) = self.empty.first_child() {
                self.empty.remove(&child);
            }
            self.empty.append(&widgets::empty_state(
                "camera-web-symbolic",
                "No cameras",
                "Cameras you add to Home Assistant show up here with their last picture.",
                None,
            ));
            self.stack.set_visible_child_name("empty");
            return;
        }
        self.stack.set_visible_child_name("content");
        // One grid; each card says its room.
        let maker = cards::maker(cameras.iter().map(|d| (*d).clone()).collect(), true);
        let key = "cameras:all";
        let g = CardGrid::new(key, maker);
        let saved = prefs::get()
            .card_order
            .get(key)
            .cloned()
            .unwrap_or_default();
        let mut default: Vec<&Device> = cameras.clone();
        default.sort_by_key(|d| (d.area.clone().unwrap_or_default(), d.name.to_lowercase()));
        let ids = grid::ordered(&saved, default.iter().map(|d| d.id.clone()).collect());
        for id in ids {
            if let Some(d) = cameras.iter().find(|d| d.id == id) {
                cards::add_to(&g, &id, cards::size_options(d, home));
            }
        }
        g.update_property(&[gtk::accessible::Property::Label("Cameras")]);
        self.flow.add_plain(key, &g);
        self.flow.set_arranging(self.arranging.get());
    }
}
