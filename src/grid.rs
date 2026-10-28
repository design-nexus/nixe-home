//! The card grid. Every card is 1, 2 or 4 units tall and wide; a row unit is
//! a fixed height and the columns stretch to fill the width, in multiples of
//! four. `CardGrid` places one section's cards densely (each in the first spot
//! it fits); `Sections` places sections side by side on the same columns,
//! each where its top is lowest. In Arrange mode cards are dragged, moved
//! with the keyboard and resized; the order and sizes are saved.
use crate::prefs;
use gtk::{gdk, glib, prelude::*, subclass::prelude::*};
use nyx_ui::widgets;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

/// Height of one row unit.
pub const ROW: i32 = 64;
pub const GAP: i32 = 12;
/// The narrowest a column gets.
const MIN_COL: i32 = 68;
/// Space between sections stacked one above another.
const SECTION_GAP: i32 = 22;

/// A card's size in units: `h` rows by `w` columns, each 1, 2 or 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub h: u8,
    pub w: u8,
}

impl Size {
    pub const fn new(h: u8, w: u8) -> Self {
        Self { h, w }
    }

    /// `"2x4"` as saved in preferences.
    pub fn key(self) -> String {
        format!("{}x{}", self.h, self.w)
    }

    pub fn parse(text: &str) -> Option<Self> {
        let (h, w) = text.split_once('x')?;
        let (h, w) = (h.parse().ok()?, w.parse().ok()?);
        ([1, 2, 4].contains(&h) && [1, 2, 4].contains(&w)).then_some(Self { h, w })
    }

    /// "2 × 4", as shown on the size button.
    pub fn label(self) -> String {
        format!("{} × {}", self.h, self.w)
    }

    /// Pixel height.
    pub fn height(self) -> i32 {
        span(self.h as usize, ROW as f64)
    }
}

/// Pixels covered by `n` units of `unit` with gaps between.
fn span(n: usize, unit: f64) -> i32 {
    (n as f64 * unit + (n.saturating_sub(1)) as f64 * GAP as f64).round() as i32
}

/// Columns for `width`: as many as fit, rounded down to a multiple of four.
pub fn columns(width: i32) -> usize {
    let fit = ((width + GAP) / (MIN_COL + GAP)).max(4) as usize;
    fit - fit % 4
}

/// The width of one column when `cols` share `width`.
fn unit(width: i32, cols: usize) -> f64 {
    ((width - GAP * (cols as i32 - 1)) as f64 / cols as f64).max(1.0)
}

/// Dense placement: each size, in order, goes to the first free spot (top to
/// bottom, left to right) where it fits. Returns `(column, row)` for each
/// and the rows used.
pub fn place(sizes: &[Size], cols: usize) -> (Vec<(usize, usize)>, usize) {
    let mut taken: Vec<Vec<bool>> = Vec::new();
    let mut out = Vec::with_capacity(sizes.len());
    for size in sizes {
        let (h, w) = (size.h as usize, (size.w as usize).min(cols));
        let mut row = 0;
        let spot = loop {
            while taken.len() < row + h {
                taken.push(vec![false; cols]);
            }
            let free = (0..=cols - w)
                .find(|&c| (row..row + h).all(|r| (c..c + w).all(|cc| !taken[r][cc])));
            if let Some(c) = free {
                break (c, row);
            }
            row += 1;
        };
        for row in taken.iter_mut().skip(spot.1).take(h) {
            row[spot.0..spot.0 + w].fill(true);
        }
        out.push(spot);
    }
    let rows = out
        .iter()
        .zip(sizes)
        .map(|((_, r), s)| r + s.h as usize)
        .max()
        .unwrap_or(0);
    (out, rows)
}

/// How many columns (a multiple of four) a section of `sizes` takes out of
/// `cols`: the narrowest that is still as short as the full width.
pub fn section_cols(sizes: &[Size], cols: usize) -> usize {
    if sizes.is_empty() {
        return 4;
    }
    let best = place(sizes, cols).1;
    (4..=cols)
        .step_by(4)
        .find(|&c| place(sizes, c).1 <= best)
        .unwrap_or(cols)
}

/// Sections of `(columns, height)` placed in order, each at the column (a
/// multiple of four) where its top is lowest. Returns `(column, y)` for each.
pub fn skyline(sections: &[(usize, i32)], cols: usize) -> Vec<(usize, i32)> {
    let mut sky = vec![0; cols];
    sections
        .iter()
        .map(|&(w, h)| {
            let w = w.clamp(1, cols);
            let (x, top) = (0..=cols - w)
                .step_by(4)
                .map(|x| (x, sky[x..x + w].iter().copied().max().unwrap_or(0)))
                .min_by_key(|&(x, top)| (top, x))
                .unwrap_or((0, 0));
            for s in &mut sky[x..x + w] {
                *s = top + h + SECTION_GAP;
            }
            (x, top)
        })
        .collect()
}

/// Widen sections into free columns to their right: placed sections are
/// `(column, y, columns, height)`; a section takes four more columns at a time
/// while no other section is there for its whole height.
pub fn stretch(placed: &mut [(usize, i32, usize, i32)], cols: usize) {
    for i in 0..placed.len() {
        loop {
            let (x, y, w, h) = placed[i];
            if x + w + 4 > cols {
                break;
            }
            let nx = x + w;
            let blocked = placed.iter().enumerate().any(|(j, &(ox, oy, ow, oh))| {
                j != i && ox < nx + 4 && nx < ox + ow && oy < y + h && y < oy + oh
            });
            if blocked {
                break;
            }
            placed[i].2 += 4;
        }
    }
}

/// `ids` in their saved order; ones without a saved place keep their order
/// after the rest.
pub fn ordered(saved: &[String], ids: Vec<String>) -> Vec<String> {
    let mut ids = ids;
    ids.sort_by_key(|id| saved.iter().position(|s| s == id).unwrap_or(usize::MAX));
    ids
}

/// Fold a new order of some of the ids (a room's cards) into the saved
/// order of all of them: the moved ids take the places they held.
pub fn merge(saved: &[String], shown: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(saved.len() + shown.len());
    let mut next = shown.iter();
    for id in saved {
        if shown.contains(id) {
            if let Some(s) = next.next() {
                out.push(s.clone());
            }
        } else {
            out.push(id.clone());
        }
    }
    out.extend(next.cloned());
    out
}

/// The saved size of a card, if it's one the card offers.
pub fn saved_size(id: &str, options: &[Size]) -> Option<Size> {
    prefs::get()
        .card_sizes
        .get(id)
        .and_then(|s| Size::parse(s))
        .filter(|s| options.contains(s))
}

/// Save the order of a grid's cards under `key`.
fn save_order(key: &str, ids: &[String]) {
    prefs::update(|p| {
        if key == "overview:favorites" {
            p.favorite_devices = merge(&p.favorite_devices, ids);
        } else {
            let saved = p.card_order.get(key).cloned().unwrap_or_default();
            let mut all = merge(&saved, ids);
            for id in ids {
                if !all.contains(id) {
                    all.push(id.clone());
                }
            }
            p.card_order.insert(key.to_string(), all);
        }
    });
}

thread_local! {
    /// The card being dragged in Arrange mode.
    static DRAGGING: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Builds a card at a size.
pub type Maker = Rc<dyn Fn(&str, Size) -> gtk::Widget>;

pub struct Entry {
    pub id: String,
    pub size: Size,
    pub holder: gtk::Overlay,
    shield: gtk::Box,
    size_label: gtk::Label,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CardGrid {
        pub key: RefCell<String>,
        pub entries: RefCell<Vec<Entry>>,
        pub maker: RefCell<Option<Maker>>,
        pub arranging: Cell<bool>,
        /// Each shown card's rectangle from the last allocation.
        pub rects: RefCell<Vec<(String, gdk::Rectangle)>>,
        /// The card last swapped with while dragging, so it doesn't swap back.
        pub last_target: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CardGrid {
        const NAME: &'static str = "NyxHomeCardGrid";
        type Type = super::CardGrid;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CardGrid {
        fn dispose(&self) {
            self.entries.borrow_mut().clear();
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl CardGrid {
        pub fn shown(&self) -> Vec<(gtk::Widget, Size, String)> {
            self.entries
                .borrow()
                .iter()
                .filter(|e| e.holder.should_layout())
                .map(|e| (e.holder.clone().upcast(), e.size, e.id.clone()))
                .collect()
        }
    }

    impl WidgetImpl for CardGrid {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let shown = self.shown();
            if shown.is_empty() {
                return (0, 0, -1, -1);
            }
            if orientation == gtk::Orientation::Horizontal {
                let min = span(4, MIN_COL as f64);
                return (min, min, -1, -1);
            }
            let width = if for_size < 0 {
                span(4, MIN_COL as f64)
            } else {
                for_size
            };
            let sizes: Vec<Size> = shown.iter().map(|s| s.1).collect();
            let rows = place(&sizes, columns(width)).1;
            let h = span(rows, ROW as f64);
            (h, h, -1, -1)
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            let shown = self.shown();
            let cols = columns(width);
            let u = unit(width, cols);
            let sizes: Vec<Size> = shown.iter().map(|s| s.1).collect();
            let (spots, _) = place(&sizes, cols);
            let mut rects = Vec::with_capacity(shown.len());
            for ((holder, size, id), (c, r)) in shown.iter().zip(spots) {
                let x = (c as f64 * (u + GAP as f64)).round() as i32;
                let y = r as i32 * (ROW + GAP);
                let w = span((size.w as usize).min(cols), u);
                let h = size.height();
                // GTK wants every child measured before it's placed.
                holder.measure(gtk::Orientation::Horizontal, -1);
                holder.measure(gtk::Orientation::Vertical, w);
                holder.size_allocate(&gtk::Allocation::new(x, y, w, h), -1);
                rects.push((id.clone(), gdk::Rectangle::new(x, y, w, h)));
            }
            *self.rects.borrow_mut() = rects;
        }
    }

    #[derive(Default)]
    pub struct Sections {
        pub key: RefCell<String>,
        /// Each section's key, its box and its cards.
        pub sections: RefCell<Vec<(String, gtk::Widget, super::CardGrid)>>,
        pub arranging: Cell<bool>,
        /// Buttons that only show in Arrange mode.
        pub arrange_only: RefCell<Vec<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Sections {
        const NAME: &'static str = "NyxHomeSections";
        type Type = super::Sections;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Sections {
        fn dispose(&self) {
            self.sections.borrow_mut().clear();
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl Sections {
        /// Each shown section with its x, y, width and height.
        pub fn layout(&self, width: i32) -> Vec<(gtk::Widget, i32, i32, i32, i32)> {
            let cols = columns(width);
            let u = unit(width, cols);
            let shown: Vec<(gtk::Widget, usize)> = self
                .sections
                .borrow()
                .iter()
                .filter(|(_, s, _)| s.should_layout())
                .map(|(_, s, g)| {
                    let sizes: Vec<Size> = g.imp().shown().iter().map(|x| x.1).collect();
                    (s.clone(), section_cols(&sizes, cols))
                })
                .collect();
            let measured: Vec<(usize, i32, i32)> = shown
                .iter()
                .map(|(s, c)| {
                    let w = span(*c, u);
                    s.measure(gtk::Orientation::Horizontal, -1);
                    (*c, w, s.measure(gtk::Orientation::Vertical, w).1)
                })
                .collect();
            let spots = skyline(
                &measured
                    .iter()
                    .map(|(c, _, h)| (*c, *h))
                    .collect::<Vec<_>>(),
                cols,
            );
            let mut placed: Vec<(usize, i32, usize, i32)> = spots
                .iter()
                .zip(&measured)
                .map(|(&(x, y), &(c, _, h))| (x, y, c, h))
                .collect();
            stretch(&mut placed, cols);
            shown
                .into_iter()
                .zip(placed)
                .map(|((s, _), (x, y, c, _))| {
                    let px = (x as f64 * (u + GAP as f64)).round() as i32;
                    let w = span(c, u);
                    // A wider section may be shorter.
                    let h = s.measure(gtk::Orientation::Vertical, w).1;
                    (s, px, y, w, h)
                })
                .collect()
        }
    }

    impl WidgetImpl for Sections {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            if orientation == gtk::Orientation::Horizontal {
                let min = span(4, MIN_COL as f64);
                return (min, min, -1, -1);
            }
            let width = if for_size < 0 {
                span(4, MIN_COL as f64)
            } else {
                for_size
            };
            let bottom = self
                .layout(width)
                .iter()
                .map(|(_, _, y, _, h)| y + h)
                .max()
                .unwrap_or(0);
            (bottom, bottom, -1, -1)
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            for (s, x, y, w, h) in self.layout(width) {
                s.size_allocate(&gtk::Allocation::new(x, y, w, h), -1);
            }
        }
    }
}

glib::wrapper! {
    pub struct CardGrid(ObjectSubclass<imp::CardGrid>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl CardGrid {
    /// A grid whose order is saved under `key` (for example `rooms:light`).
    pub fn new(key: &str, maker: Maker) -> Self {
        let g: Self = glib::Object::new();
        g.set_accessible_role(gtk::AccessibleRole::List);
        *g.imp().key.borrow_mut() = key.to_string();
        *g.imp().maker.borrow_mut() = Some(maker);
        g.add_controller(g.drop_target());
        g
    }

    /// Add a card for `id` at `size`; `options` are the sizes it offers.
    pub fn add(&self, id: &str, size: Size, options: Vec<Size>) {
        let card = self.make(id, size);
        let holder = gtk::Overlay::new();
        holder.set_child(Some(&card));
        holder.set_widget_name(id);
        let shield = widgets::hbox(6);
        shield.add_css_class("arrange-shield");
        shield.set_focusable(true);
        shield.set_visible(self.imp().arranging.get());
        let handle = gtk::Image::from_icon_name("list-drag-handle-symbolic");
        handle.add_css_class("arrange-handle");
        handle.set_halign(gtk::Align::Start);
        handle.set_valign(gtk::Align::Start);
        handle.set_hexpand(true);
        shield.append(&handle);
        let size_button = gtk::MenuButton::new();
        size_button.add_css_class("size-pill");
        size_button.set_halign(gtk::Align::End);
        size_button.set_valign(gtk::Align::Start);
        let size_label = gtk::Label::new(Some(&size.label()));
        size_button.set_child(Some(&size_label));
        size_button.set_tooltip_text(Some("Card size (Ctrl+S)"));
        size_button.update_property(&[gtk::accessible::Property::Label("Card size")]);
        size_button.set_sensitive(options.len() > 1);
        shield.append(&size_button);
        holder.add_overlay(&shield);
        holder.set_parent(self);
        self.wire_shield(id, &shield, &size_button, &options);
        self.imp().entries.borrow_mut().push(Entry {
            id: id.to_string(),
            size,
            holder,
            shield,
            size_label,
        });
    }

    fn make(&self, id: &str, size: Size) -> gtk::Widget {
        let maker = self.imp().maker.borrow().clone();
        maker.map_or_else(
            || gtk::Box::new(gtk::Orientation::Vertical, 0).upcast(),
            |m| m(id, size),
        )
    }

    pub fn ids(&self) -> Vec<String> {
        self.imp()
            .entries
            .borrow()
            .iter()
            .map(|e| e.id.clone())
            .collect()
    }

    /// Show or hide one card (search).
    pub fn set_shown(&self, id: &str, shown: bool) {
        if let Some(e) = self.imp().entries.borrow().iter().find(|e| e.id == id) {
            e.holder.set_visible(shown);
        }
    }

    pub fn shown_count(&self) -> usize {
        self.imp().shown().len()
    }

    pub fn set_arranging(&self, on: bool) {
        self.imp().arranging.set(on);
        for e in self.imp().entries.borrow().iter() {
            e.shield.set_visible(on);
            if let Some(card) = e.holder.child() {
                card.set_can_target(!on);
                card.set_can_focus(!on);
            }
        }
    }

    /// Move a card `by` places and save the order.
    fn shift(&self, id: &str, by: isize) {
        {
            let mut entries = self.imp().entries.borrow_mut();
            let Some(i) = entries.iter().position(|e| e.id == id) else {
                return;
            };
            let j = (i as isize + by).clamp(0, entries.len() as isize - 1) as usize;
            let e = entries.remove(i);
            entries.insert(j, e);
        }
        self.queue_allocate();
        self.save();
    }

    /// Move `id` to where `target` is.
    fn move_to(&self, id: &str, target: &str) {
        {
            let mut entries = self.imp().entries.borrow_mut();
            let (Some(i), Some(j)) = (
                entries.iter().position(|e| e.id == id),
                entries.iter().position(|e| e.id == target),
            ) else {
                return;
            };
            let e = entries.remove(i);
            entries.insert(j, e);
        }
        self.queue_resize();
    }

    fn save(&self) {
        let shown: Vec<String> = self.imp().shown().into_iter().map(|s| s.2).collect();
        save_order(&self.imp().key.borrow(), &shown);
    }

    /// Rebuild a card at a new size and remember it.
    fn resize_card(&self, id: &str, size: Size) {
        let card = self.make(id, size);
        card.set_can_target(!self.imp().arranging.get());
        if let Some(e) = self
            .imp()
            .entries
            .borrow_mut()
            .iter_mut()
            .find(|e| e.id == id)
        {
            e.size = size;
            e.size_label.set_text(&size.label());
            e.holder.set_child(Some(&card));
        }
        prefs::update(|p| {
            p.card_sizes.insert(id.to_string(), size.key());
        });
        self.queue_resize();
    }

    fn wire_shield(
        &self,
        id: &str,
        shield: &gtk::Box,
        size_button: &gtk::MenuButton,
        options: &[Size],
    ) {
        // Sizes.
        let popover = gtk::Popover::new();
        popover.add_css_class("menu-popover");
        let items = widgets::vbox(0);
        for size in options {
            let b = gtk::Button::with_label(&size.label());
            b.add_css_class("flat");
            b.add_css_class("menu-item");
            let (me, id, size, p) = (self.downgrade(), id.to_string(), *size, popover.clone());
            b.connect_clicked(move |_| {
                p.popdown();
                if let Some(me) = me.upgrade() {
                    me.resize_card(&id, size);
                }
            });
            items.append(&b);
        }
        popover.set_child(Some(&items));
        size_button.set_popover(Some(&popover));

        // Keyboard: Ctrl+arrows move, Ctrl+S picks a size.
        let keys = gtk::EventControllerKey::new();
        let (me, key_id, sb) = (self.downgrade(), id.to_string(), size_button.clone());
        keys.connect_key_pressed(move |_, key, _, mods| {
            if !mods.contains(gdk::ModifierType::CONTROL_MASK) {
                return glib::Propagation::Proceed;
            }
            let Some(me) = me.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match key {
                gdk::Key::Left | gdk::Key::Up => me.shift(&key_id, -1),
                gdk::Key::Right | gdk::Key::Down => me.shift(&key_id, 1),
                gdk::Key::s => sb.popup(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        shield.add_controller(keys);
        shield.update_property(&[gtk::accessible::Property::Label(&format!(
            "Move card {id}: Control and arrow keys"
        ))]);

        // Dragging.
        let drag = gtk::DragSource::new();
        drag.set_actions(gdk::DragAction::MOVE);
        let drag_id = id.to_string();
        drag.connect_prepare(move |_, _, _| {
            Some(gdk::ContentProvider::for_value(&drag_id.to_value()))
        });
        let (me, begin_id) = (self.downgrade(), id.to_string());
        drag.connect_drag_begin(move |src, _| {
            DRAGGING.with(|d| *d.borrow_mut() = Some(begin_id.clone()));
            if let Some(me) = me.upgrade()
                && let Some(e) = me.imp().entries.borrow().iter().find(|e| e.id == begin_id)
            {
                let icon = gtk::WidgetPaintable::new(Some(&e.holder));
                src.set_icon(Some(&icon), 24, 24);
                e.holder.add_css_class("dragging");
            }
        });
        let (me, end_id) = (self.downgrade(), id.to_string());
        drag.connect_drag_end(move |_, _, _| {
            DRAGGING.with(|d| *d.borrow_mut() = None);
            if let Some(me) = me.upgrade() {
                if let Some(e) = me.imp().entries.borrow().iter().find(|e| e.id == end_id) {
                    e.holder.remove_css_class("dragging");
                }
                *me.imp().last_target.borrow_mut() = None;
                me.save();
            }
        });
        shield.add_controller(drag);
    }

    /// Cards follow the pointer while one of this grid's cards is dragged.
    fn drop_target(&self) -> gtk::DropTarget {
        let target = gtk::DropTarget::new(glib::Type::STRING, gdk::DragAction::MOVE);
        let me = self.downgrade();
        target.connect_motion(move |_, x, y| {
            let Some(me) = me.upgrade() else {
                return gdk::DragAction::empty();
            };
            let Some(dragged) = DRAGGING.with(|d| d.borrow().clone()) else {
                return gdk::DragAction::empty();
            };
            if !me.ids().contains(&dragged) {
                return gdk::DragAction::empty();
            }
            let over = me
                .imp()
                .rects
                .borrow()
                .iter()
                .find(|(_, r)| r.contains_point(x as i32, y as i32))
                .map(|(id, _)| id.clone());
            let last = me.imp().last_target.borrow().clone();
            match over {
                Some(over) if over != dragged && last.as_ref() != Some(&over) => {
                    me.move_to(&dragged, &over);
                    *me.imp().last_target.borrow_mut() = Some(over);
                }
                Some(over) if over == dragged => {
                    *me.imp().last_target.borrow_mut() = None;
                }
                _ => {}
            }
            gdk::DragAction::MOVE
        });
        target.connect_drop(|_, _, _, _| true);
        target
    }
}

glib::wrapper! {
    pub struct Sections(ObjectSubclass<imp::Sections>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Sections {
    /// Sections whose order is saved under `key` (for example `rooms`).
    pub fn new(key: &str) -> Self {
        let s: Self = glib::Object::new();
        *s.imp().key.borrow_mut() = key.to_string();
        s
    }

    /// The saved order of section keys.
    pub fn order(key: &str, keys: Vec<String>) -> Vec<String> {
        let saved = prefs::get()
            .section_order
            .get(key)
            .cloned()
            .unwrap_or_default();
        ordered(&saved, keys)
    }

    /// Add a section: a heading (with optional extra buttons) over `grid`.
    pub fn add(
        &self,
        key: &str,
        title: &gtk::Label,
        extra: Option<&gtk::Widget>,
        grid: &CardGrid,
    ) -> gtk::Box {
        let section = widgets::vbox(8);
        section.add_css_class("type-section");
        let head = widgets::hbox(4);
        head.add_css_class("section-head");
        title.set_hexpand(true);
        head.append(title);
        if let Some(extra) = extra {
            head.append(extra);
        }
        for (icon, tip, by) in [
            ("go-previous-symbolic", "Move section earlier", -1isize),
            ("go-next-symbolic", "Move section later", 1),
        ] {
            let b = widgets::bar_button(icon, tip);
            b.update_property(&[gtk::accessible::Property::Label(tip)]);
            b.set_visible(self.imp().arranging.get());
            let (me, key) = (self.downgrade(), key.to_string());
            b.connect_clicked(move |_| {
                if let Some(me) = me.upgrade() {
                    me.shift(&key, by);
                }
            });
            head.append(&b);
            self.imp().arrange_only.borrow_mut().push(b.upcast());
        }
        section.append(&head);
        section.append(grid);
        grid.set_arranging(self.imp().arranging.get());
        section.set_parent(self);
        self.imp().sections.borrow_mut().push((
            key.to_string(),
            section.clone().upcast(),
            grid.clone(),
        ));
        section
    }

    /// Add cards with no heading: the whole page is one grid.
    pub fn add_plain(&self, key: &str, grid: &CardGrid) {
        grid.set_arranging(self.imp().arranging.get());
        grid.set_parent(self);
        self.imp().sections.borrow_mut().push((
            key.to_string(),
            grid.clone().upcast(),
            grid.clone(),
        ));
    }

    pub fn clear(&self) {
        self.imp().sections.borrow_mut().clear();
        self.imp().arrange_only.borrow_mut().clear();
        while let Some(child) = self.first_child() {
            child.unparent();
        }
    }

    pub fn grids(&self) -> Vec<CardGrid> {
        self.imp()
            .sections
            .borrow()
            .iter()
            .map(|s| s.2.clone())
            .collect()
    }

    /// Every section's box, with its key and cards.
    pub fn sections(&self) -> Vec<(String, gtk::Widget, CardGrid)> {
        self.imp().sections.borrow().clone()
    }

    pub fn set_arranging(&self, on: bool) {
        self.imp().arranging.set(on);
        for b in self.imp().arrange_only.borrow().iter() {
            b.set_visible(on);
        }
        for g in self.grids() {
            g.set_arranging(on);
        }
    }

    fn shift(&self, key: &str, by: isize) {
        {
            let mut sections = self.imp().sections.borrow_mut();
            let Some(i) = sections.iter().position(|s| s.0 == key) else {
                return;
            };
            let j = (i as isize + by).clamp(0, sections.len() as isize - 1) as usize;
            let s = sections.remove(i);
            sections.insert(j, s);
        }
        let keys: Vec<String> = self
            .imp()
            .sections
            .borrow()
            .iter()
            .map(|s| s.0.clone())
            .collect();
        let page = self.imp().key.borrow().clone();
        prefs::update(|p| {
            p.section_order.insert(page.clone(), keys.clone());
        });
        self.queue_resize();
    }
}

/// Forget a page's saved order and sizes (`prefix` like `rooms:`), for the
/// cards in `ids`.
pub fn reset(page: &str, ids: &[String]) {
    let prefix = format!("{page}:");
    prefs::update(|p| {
        p.card_order.retain(|k, _| !k.starts_with(&prefix));
        p.section_order.remove(page);
        for id in ids {
            p.card_sizes.remove(id);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const S14: Size = Size::new(1, 4);
    const S24: Size = Size::new(2, 4);
    const S44: Size = Size::new(4, 4);
    const S22: Size = Size::new(2, 2);

    #[test]
    fn columns_come_in_fours() {
        assert_eq!(columns(1760), 20);
        assert_eq!(columns(760), 8);
        assert_eq!(columns(300), 4);
    }

    #[test]
    fn small_cards_fill_the_holes() {
        // A 4x4 thermostat, then two toggles fill the column beside it.
        let (spots, rows) = place(&[S44, S14, S14, S24], 8);
        assert_eq!(spots, [(0, 0), (4, 0), (4, 1), (4, 2)]);
        assert_eq!(rows, 4);
        let (spots, _) = place(&[S22, S22, S22], 4);
        assert_eq!(spots, [(0, 0), (2, 0), (0, 2)]);
    }

    #[test]
    fn sections_take_the_narrowest_width_that_is_as_short() {
        assert_eq!(section_cols(&[S24; 7], 20), 16);
        assert_eq!(section_cols(&[S44, S44], 20), 8);
        assert_eq!(section_cols(&[S24], 20), 4);
    }

    #[test]
    fn short_sections_slide_under_others() {
        // Lights 12 wide and tall, climate 4 wide and tall, security 4 wide
        // and short, then locks go under security.
        let spots = skyline(&[(12, 300), (4, 300), (4, 100), (4, 60)], 20);
        assert_eq!(spots, [(0, 0), (12, 0), (16, 0), (16, 122)]);
    }

    #[test]
    fn sections_widen_into_free_columns() {
        // Scenes 16 wide with nothing beside them take the whole width;
        // lights beside climate stay as they are.
        let mut placed = vec![(0, 0, 16, 300), (0, 322, 12, 300), (12, 322, 8, 300)];
        stretch(&mut placed, 20);
        assert_eq!(placed[0].2, 20);
        assert_eq!(placed[1].2, 12);
        assert_eq!(placed[2].2, 8);
    }

    #[test]
    fn saved_order_survives_a_room_view() {
        let saved: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        // In one room only b and d show; the user puts d first.
        let merged = merge(&saved, &["d".into(), "b".into()]);
        assert_eq!(merged, ["a", "d", "c", "b"]);
        assert_eq!(
            ordered(&saved, vec!["new".into(), "c".into(), "a".into()]),
            ["a", "c", "new"]
        );
        assert_eq!(Size::parse("2x4"), Some(S24));
        assert_eq!(Size::parse("3x4"), None);
    }
}
