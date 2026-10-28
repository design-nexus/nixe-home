//! Camera stills and robot maps: small thumbnails on cards, a zoomable map
//! view, and a window to look at them properly, with zoom, pan, refresh, live
//! video for cameras that stream, and close.
use crate::{app, ha, live, robot_map, rt};
use gtk::{gdk, glib, prelude::*};
use nyx_ui::widgets;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
};

type Done = Box<dyn FnOnce(Result<gdk::Texture, String>)>;

thread_local! {
    /// Images asked for this session, so each loads once until refreshed.
    static REQUESTED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// Loads in flight, with everyone waiting on each.
    static PENDING: RefCell<HashMap<String, Vec<Done>>> = RefCell::new(HashMap::new());
}

/// Load an entity's image from Home Assistant into the session cache, then
/// call `done`. Home Assistant answers from its own last snapshot, so this
/// doesn't wake a battery camera.
pub fn load(entity_id: &str, done: impl FnOnce(Result<gdk::Texture, String>) + 'static) {
    let Some(app) = app::get() else { return };
    let (base, token, entity) = {
        let home = app.home.borrow();
        (
            home.base.clone(),
            home.token.clone(),
            home.entities.get(entity_id).cloned(),
        )
    };
    let Some(entity) = entity else {
        done(Err("That camera is no longer in Home Assistant.".into()));
        return;
    };
    REQUESTED.with(|r| r.borrow_mut().insert(entity_id.to_string()));
    let first = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let waiting = p.entry(entity_id.to_string()).or_default();
        waiting.push(Box::new(done));
        waiting.len() == 1
    });
    if !first {
        return;
    }
    rt::spawn(
        async move {
            let bytes = ha::fetch_entity_picture(&base, &token, &entity).await;
            (entity.entity_id, bytes)
        },
        move |(id, result)| {
            let texture = result.and_then(|bytes| {
                gdk::Texture::from_bytes(&glib::Bytes::from(&bytes))
                    .map_err(|_| "Couldn't read the image.".to_string())
            });
            if let (Ok(texture), Some(app)) = (&texture, app::get()) {
                app.home
                    .borrow_mut()
                    .previews
                    .insert(id.clone(), texture.clone());
            }
            let waiting = PENDING
                .with(|p| p.borrow_mut().remove(&id))
                .unwrap_or_default();
            for done in waiting {
                done(texture.clone());
            }
        },
    );
}

fn cached(entity_id: &str) -> Option<gdk::Texture> {
    app::get().and_then(|app| app.home.borrow().previews.get(entity_id).cloned())
}

/// A fixed-height picture of an entity's image that opens the viewer. It loads
/// once per session; it never refreshes on its own.
pub fn thumbnail(entity_id: &str, title: &str, height: i32) -> gtk::Button {
    let picture = gtk::Picture::new();
    picture.add_css_class("thumb-picture");
    picture.set_can_shrink(true);
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_size_request(-1, height);
    picture.set_hexpand(true);
    let empty = widgets::label("No picture yet", "thumb-empty");
    empty.set_halign(gtk::Align::Center);
    empty.set_valign(gtk::Align::Center);
    let stack = gtk::Stack::new();
    stack.set_size_request(-1, height);
    stack.add_named(&empty, Some("empty"));
    stack.add_named(&picture, Some("picture"));
    // A picture asks for its full pixel width; this keeps the card's own width.
    let clip = gtk::ScrolledWindow::new();
    clip.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
    clip.set_propagate_natural_width(false);
    clip.set_child(Some(&stack));
    let button = gtk::Button::new();
    button.add_css_class("thumb");
    button.set_child(Some(&clip));
    button.set_overflow(gtk::Overflow::Hidden);
    button.set_tooltip_text(Some(&format!("Open {title} larger")));
    button.update_property(&[gtk::accessible::Property::Label(&format!(
        "Open {title} larger"
    ))]);
    let show = {
        let (picture, stack) = (picture.clone(), stack.clone());
        move |texture: &gdk::Texture| {
            picture.set_paintable(Some(texture));
            stack.set_visible_child_name("picture");
        }
    };
    let loading = PENDING.with(|p| p.borrow().contains_key(entity_id));
    match cached(entity_id) {
        Some(texture) => show(&texture),
        None if loading || !REQUESTED.with(|r| r.borrow().contains(entity_id)) => {
            empty.set_text("Loading…");
            let empty = empty.clone();
            load(entity_id, move |result| match result {
                Ok(texture) => show(&texture),
                Err(_) => empty.set_text("No picture yet"),
            });
        }
        None => {}
    }
    let (id, title) = (entity_id.to_string(), title.to_string());
    let picture2 = picture.clone();
    button.connect_clicked(move |_| {
        let picture = picture2.clone();
        open(
            &id,
            &title,
            Some(Rc::new(move |t: &gdk::Texture| {
                picture.set_paintable(Some(t))
            })),
        );
    });
    button
}

/// Called with the new picture after a refresh.
pub type OnRefresh = Rc<dyn Fn(&gdk::Texture)>;

/// A picture to look at properly: fitted to start, then zoom (Ctrl+scroll,
/// pinch, `zoom_by`) and drag to pan, with room for Home's layer on top.
pub struct MapView {
    /// The view, with a note over it ("Loading…").
    pub root: gtk::Overlay,
    pub note: gtk::Label,
    /// "52%", for a header.
    pub zoom_label: gtk::Label,
    picture: gtk::Picture,
    clip: gtk::ScrolledWindow,
    stage: gtk::Overlay,
    scroll: gtk::ScrolledWindow,
    /// 0 means "fit the view".
    zoom: Cell<f64>,
    size: Cell<(i32, i32)>,
    layer: RefCell<Option<Rc<robot_map::View>>>,
}

impl MapView {
    pub fn new(alt: &str) -> Rc<Self> {
        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Fill);
        picture.set_alternative_text(Some(alt));
        // A picture asks for its full pixel size and would stretch to fill
        // the view; this holds it at exactly the zoomed size.
        let clip = gtk::ScrolledWindow::new();
        clip.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Never);
        clip.set_propagate_natural_width(false);
        clip.set_propagate_natural_height(false);
        clip.set_child(Some(&picture));
        // The picture with anything drawn over it (a robot map's names and markers).
        let stage = gtk::Overlay::new();
        stage.set_child(Some(&clip));
        stage.set_halign(gtk::Align::Center);
        stage.set_valign(gtk::Align::Center);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&stage)
            .hexpand(true)
            .vexpand(true)
            .build();
        scroll.add_css_class("viewer");
        let note = widgets::label("Loading…", "dim");
        note.set_halign(gtk::Align::Center);
        note.set_valign(gtk::Align::Center);
        note.set_wrap(true);
        note.set_justify(gtk::Justification::Center);
        let root = gtk::Overlay::new();
        root.set_child(Some(&scroll));
        root.add_overlay(&note);
        let zoom_label = widgets::label("", "num");
        zoom_label.set_width_chars(5);
        let view = Rc::new(Self {
            root,
            note,
            zoom_label,
            picture,
            clip,
            stage,
            scroll,
            zoom: Cell::new(0.0),
            size: Cell::new((0, 0)),
            layer: RefCell::new(None),
        });
        view.wire();
        view
    }

    fn wire(self: &Rc<Self>) {
        // Refit while "fit" is on and the view changes size.
        let probe = gtk::DrawingArea::new();
        probe.set_can_target(false);
        let me = Rc::downgrade(self);
        probe.connect_resize(move |_, _, _| {
            if let Some(me) = me.upgrade()
                && me.zoom.get() <= 0.0
            {
                let me = me.clone();
                glib::idle_add_local_once(move || me.apply(None));
            }
        });
        self.root.add_overlay(&probe);
        self.root.set_measure_overlay(&probe, false);
        // Ctrl+scroll zooms at the pointer; plain scrolling scrolls.
        let pointer = Rc::new(Cell::new((0.0, 0.0)));
        let motion = gtk::EventControllerMotion::new();
        let p = pointer.clone();
        motion.connect_motion(move |_, x, y| p.set((x, y)));
        self.scroll.add_controller(motion);
        let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
        let me = Rc::downgrade(self);
        wheel.connect_scroll(move |c, _, dy| {
            if !c
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK)
            {
                return glib::Propagation::Proceed;
            }
            if let Some(me) = me.upgrade() {
                me.zoom_by(
                    Some(if dy < 0.0 { 1.15 } else { 1.0 / 1.15 }),
                    Some(pointer.get()),
                );
            }
            glib::Propagation::Stop
        });
        self.scroll.add_controller(wheel);
        let pinch = gtk::GestureZoom::new();
        let last = Rc::new(Cell::new(1.0));
        let l = last.clone();
        pinch.connect_begin(move |_, _| l.set(1.0));
        let me = Rc::downgrade(self);
        pinch.connect_scale_changed(move |_, scale| {
            let step = scale / last.get();
            last.set(scale);
            if let Some(me) = me.upgrade() {
                me.zoom_by(Some(step), None);
            }
        });
        self.scroll.add_controller(pinch);
        // Drag to pan.
        let drag = gtk::GestureDrag::new();
        let start = Rc::new(Cell::new((0.0, 0.0)));
        let (st, sc) = (start.clone(), self.scroll.clone());
        drag.connect_drag_begin(move |_, _, _| {
            st.set((sc.hadjustment().value(), sc.vadjustment().value()))
        });
        let sc = self.scroll.clone();
        drag.connect_drag_update(move |_, dx, dy| {
            let (x, y) = start.get();
            sc.hadjustment().set_value(x - dx);
            sc.vadjustment().set_value(y - dy);
        });
        self.scroll.add_controller(drag);
    }

    /// Size the picture for the current zoom, keeping the point under
    /// `anchor` (or the centre) where it was.
    fn apply(&self, anchor: Option<(f64, f64)>) {
        let (w, h) = self.size.get();
        if w == 0 || h == 0 {
            return;
        }
        let (vw, vh) = (
            self.scroll.width().max(1) as f64,
            self.scroll.height().max(1) as f64,
        );
        let fitted = (vw / w as f64).min(vh / h as f64);
        let z = if self.zoom.get() <= 0.0 {
            fitted
        } else {
            self.zoom.get()
        };
        let (ha, va) = (self.scroll.hadjustment(), self.scroll.vadjustment());
        let (ax, ay) = anchor.unwrap_or((vw / 2.0, vh / 2.0));
        let old_w = self.clip.width().max(1) as f64;
        let old_h = self.clip.height().max(1) as f64;
        let fx = (ha.value() + ax) / old_w;
        let fy = (va.value() + ay) / old_h;
        let (nw, nh) = ((w as f64 * z).round() as i32, (h as f64 * z).round() as i32);
        self.clip.set_size_request(nw, nh);
        if let Some(view) = self.layer.borrow().as_ref() {
            view.set_scale(z);
        }
        self.zoom_label
            .set_text(&format!("{}%", (z * 100.0).round() as i64));
        glib::idle_add_local_once(move || {
            ha.set_value(fx * nw as f64 - ax);
            va.set_value(fy * nh as f64 - ay);
        });
    }

    /// Zoom by `factor` (`None` fits the view).
    pub fn zoom_by(&self, factor: Option<f64>, anchor: Option<(f64, f64)>) {
        match factor {
            None => self.zoom.set(0.0),
            Some(f) => {
                let (w, h) = self.size.get();
                let fitted = (self.scroll.width().max(1) as f64 / w.max(1) as f64)
                    .min(self.scroll.height().max(1) as f64 / h.max(1) as f64);
                let current = if self.zoom.get() <= 0.0 {
                    fitted
                } else {
                    self.zoom.get()
                };
                self.zoom.set((current * f).clamp(0.05, 8.0));
            }
        }
        self.apply(anchor);
    }

    /// Show at its own size (100%).
    pub fn actual(&self) {
        self.zoom.set(1.0);
        self.apply(None);
    }

    /// Show `paintable`, `size` pixels big, keeping the zoom.
    pub fn show(self: &Rc<Self>, paintable: &impl IsA<gdk::Paintable>, size: (i32, i32)) {
        let resized = self.size.replace(size) != size;
        self.picture.set_paintable(Some(paintable));
        self.note.set_visible(false);
        if resized {
            let me = self.clone();
            glib::idle_add_local_once(move || me.apply(None));
        }
    }

    pub fn show_texture(self: &Rc<Self>, texture: &gdk::Texture) {
        self.show(texture, (texture.width(), texture.height()));
    }

    /// Put Home's layer on top when `camera` is a robot's map.
    pub fn attach_layer(&self, camera: &str) {
        *self.layer.borrow_mut() = robot_map::attach(camera, &self.stage);
    }

    /// +, -, 0 (fit) and 1 (100%) on `widget`.
    pub fn keys(self: &Rc<Self>, widget: &impl IsA<gtk::Widget>) {
        let keys = gtk::EventControllerKey::new();
        let me = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, mods| {
            let Some(me) = me.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if mods.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            match key {
                gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => me.zoom_by(Some(1.25), None),
                gdk::Key::minus | gdk::Key::KP_Subtract => me.zoom_by(Some(1.0 / 1.25), None),
                gdk::Key::_0 => me.zoom_by(None, None),
                gdk::Key::_1 => me.actual(),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
        widget.add_controller(keys);
    }
}

/// Zoom buttons for a header: out, the zoom, in, Fit, 100%.
pub fn zoom_buttons(view: &Rc<MapView>, header: &gtk::Box) {
    let out = widgets::bar_button("zoom-out-symbolic", "Zoom out (-)");
    let inn = widgets::bar_button("zoom-in-symbolic", "Zoom in (+)");
    let fit = gtk::Button::with_label("Fit");
    fit.set_tooltip_text(Some("Fit to the window (0)"));
    let actual = gtk::Button::with_label("100%");
    actual.set_tooltip_text(Some("Actual size (1)"));
    header.append(&out);
    header.append(&view.zoom_label);
    header.append(&inn);
    header.append(&fit);
    header.append(&actual);
    let v = view.clone();
    out.connect_clicked(move |_| v.zoom_by(Some(1.0 / 1.25), None));
    let v = view.clone();
    inn.connect_clicked(move |_| v.zoom_by(Some(1.25), None));
    let v = view.clone();
    fit.connect_clicked(move |_| v.zoom_by(None, None));
    let v = view.clone();
    actual.connect_clicked(move |_| v.actual());
}

/// The camera to stream for `camera`: its `<id>_live` twin when there is one.
/// Some integrations (the Eufy bridge) give Home Assistant no stream; a
/// Generic Camera on the bridge's RTSP stream, named like this, stands in.
pub fn stream_of(
    camera: &str,
    entities: &std::collections::BTreeMap<String, ha::Entity>,
) -> String {
    let twin = format!("{camera}_live");
    if entities.contains_key(&twin) {
        twin
    } else {
        camera.to_string()
    }
}

/// Whether Home Assistant can stream `entity` live.
pub fn streams(entity: &ha::Entity) -> bool {
    entity.domain() == "camera"
        && entity.attributes["supported_features"]
            .as_u64()
            .unwrap_or(0)
            & 2
            != 0
        && !matches!(entity.state.as_str(), "unavailable" | "off")
}

/// How long live video plays before asking, and how long it waits for an
/// answer or a first frame.
const ASK_AFTER: u32 = 180;
const ANSWER_WITHIN: u32 = 15;
const FIRST_FRAME_WITHIN: u32 = 30;

/// Live video in a viewer: the toggle, mute, the "Still watching?" banner and
/// the timers, which a newer start or stop makes stale.
struct Live {
    camera: String,
    /// What streams: the camera, or its `_live` twin (see [`stream_of`]).
    stream: String,
    view: Rc<MapView>,
    toggle: gtk::ToggleButton,
    mute: gtk::ToggleButton,
    banner: gtk::Box,
    on: Cell<bool>,
    showing: Cell<bool>,
    /// No "Still watching?" for a camera that's plugged in.
    plugged: bool,
    /// Bumped on every start and stop, so old timers know they're stale.
    session: Cell<u32>,
    handlers: RefCell<Vec<glib::SignalHandlerId>>,
    on_refresh: Option<OnRefresh>,
    /// Quiet while setting the toggle from code.
    quiet: Cell<bool>,
}

impl Live {
    fn start(self: &Rc<Self>) {
        if self.on.replace(true) {
            return;
        }
        let session = self.session.get().wrapping_add(1);
        self.session.set(session);
        self.showing.set(false);
        self.mute.set_visible(true);
        self.mute.set_active(false);
        live::set_muted(true);
        self.view.note.set_text("Waking the camera…");
        self.toggle.add_css_class("waking");
        self.view.note.set_visible(true);
        let paintable = live::paintable();
        let me = Rc::downgrade(self);
        let frame = paintable.connect_invalidate_contents(move |p| {
            if let Some(me) = me.upgrade()
                && me.on.get()
                && p.has_frame()
            {
                let size = (p.intrinsic_width(), p.intrinsic_height());
                if !me.showing.replace(true) || size != me.view.size.get() {
                    me.view.show(p, size);
                }
                me.toggle.remove_css_class("waking");
                me.toggle.add_css_class("streaming");
            }
        });
        self.handlers.borrow_mut().push(frame);
        let me = Rc::downgrade(self);
        live::play(&self.stream, move |why| {
            if let Some(me) = me.upgrade() {
                me.stop(Some(why));
            }
        });
        // No picture in time: give up rather than keep the camera awake.
        self.after(session, FIRST_FRAME_WITHIN, |me| {
            if !me.showing.get() {
                me.stop(Some("The camera didn't start streaming.".into()));
            }
        });
        self.countdown(session);
    }

    /// Ask "Still watching?" after a while; stop if nobody answers.
    fn countdown(self: &Rc<Self>, session: u32) {
        if self.plugged {
            return;
        }
        self.after(session, ASK_AFTER, move |me| {
            me.banner.set_visible(true);
            let asked = me.session.get();
            me.after(asked, ANSWER_WITHIN, |me| {
                if me.banner.is_visible() {
                    me.stop(Some(
                        "Live video stopped to save the camera's battery.".into(),
                    ));
                }
            });
        });
    }

    fn keep_watching(self: &Rc<Self>) {
        self.banner.set_visible(false);
        let session = self.session.get().wrapping_add(1);
        self.session.set(session);
        self.countdown(session);
    }

    fn after(self: &Rc<Self>, session: u32, seconds: u32, f: impl FnOnce(&Rc<Self>) + 'static) {
        let me = Rc::downgrade(self);
        glib::timeout_add_seconds_local_once(seconds, move || {
            if let Some(me) = me.upgrade()
                && me.on.get()
                && me.session.get() == session
            {
                f(&me);
            }
        });
    }

    /// Stop the video and go back to a still, a fresh one since the camera
    /// is awake anyway. `why` is said when it wasn't the user's choice.
    fn stop(self: &Rc<Self>, why: Option<String>) {
        if !self.on.replace(false) {
            return;
        }
        self.session.set(self.session.get().wrapping_add(1));
        live::stop(&self.stream);
        let paintable = live::paintable();
        for h in self.handlers.borrow_mut().drain(..) {
            paintable.disconnect(h);
        }
        self.banner.set_visible(false);
        self.mute.set_visible(false);
        self.toggle.remove_css_class("waking");
        self.toggle.remove_css_class("streaming");
        self.quiet.set(true);
        self.toggle.set_active(false);
        self.quiet.set(false);
        if let Some(why) = why {
            app::toast(&why);
        }
        if let Some(texture) = cached(&self.camera) {
            self.view.show_texture(&texture);
        }
        let (view, on_refresh) = (self.view.clone(), self.on_refresh.clone());
        load(&self.camera, move |result| {
            if let Ok(texture) = result {
                view.show_texture(&texture);
                if let Some(f) = &on_refresh {
                    f(&texture);
                }
            }
        });
    }
}

/// The viewer window: the image fitted to the window to start, then zoom
/// (buttons, Ctrl+scroll, pinch, + and -), pan by dragging, Refresh, Live for
/// cameras that stream, and close with the button or Esc.
pub fn open(entity_id: &str, title: &str, on_refresh: Option<OnRefresh>) {
    open_with(entity_id, title, on_refresh, false);
}

/// Developer aid: open a camera straight into live video.
pub fn open_live(entity_id: &str) {
    open_with(entity_id, entity_id, None, true);
}

fn open_with(entity_id: &str, title: &str, on_refresh: Option<OnRefresh>, start_live: bool) {
    let Some(app) = app::get() else { return };
    let can_stream = app
        .home
        .borrow()
        .entities
        .get(entity_id)
        .is_some_and(streams);
    // Plugged-in cameras don't need live video to stop to save a battery.
    let plugged = app
        .home
        .borrow()
        .device_list(true)
        .iter()
        .any(|d| d.plugged && d.entities.iter().any(|e| e == entity_id));
    let window = gtk::Window::builder()
        .title(title)
        .transient_for(&app.window)
        .default_width((app.window.width() as f64 * 0.85).max(640.0) as i32)
        .default_height((app.window.height() as f64 * 0.85).max(480.0) as i32)
        .build();
    window.set_application(app.window.application().as_ref());
    window.add_css_class("home-window");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

    let header = nyx_ui::chrome::app_header(&window, "camera-photo-symbolic", title);
    header.end.set_spacing(4);
    let view = MapView::new(title);
    let toggle = gtk::ToggleButton::new();
    let mute = gtk::ToggleButton::new();
    if can_stream {
        let content = widgets::hbox(6);
        content.append(&gtk::Image::from_icon_name("media-record-symbolic"));
        content.append(&gtk::Label::new(Some("Live")));
        toggle.set_child(Some(&content));
        toggle.add_css_class("live-toggle");
        toggle.set_tooltip_text(Some(
            "Watch live (L). Stops on its own after a few minutes.",
        ));
        toggle.update_property(&[gtk::accessible::Property::Label("Watch live")]);
        header.end.append(&toggle);
        mute.set_icon_name("audio-volume-muted-symbolic");
        mute.add_css_class("bar-button");
        mute.set_tooltip_text(Some("Sound (M)"));
        mute.update_property(&[gtk::accessible::Property::Label("Sound")]);
        mute.set_visible(false);
        header.end.append(&mute);
    }
    zoom_buttons(&view, &header.end);
    let refresh = widgets::bar_button(
        "view-refresh-symbolic",
        "Get a new picture from Home Assistant",
    );
    let close = gtk::Button::with_label("Close");
    close.set_tooltip_text(Some("Close (Esc)"));
    header.end.append(&refresh);
    header.end.append(&close);

    // "Still watching?" over the bottom of the picture.
    let banner = widgets::hbox(10);
    banner.add_css_class("live-banner");
    banner.set_halign(gtk::Align::Center);
    banner.set_valign(gtk::Align::End);
    banner.set_margin_bottom(20);
    banner.append(&widgets::label("Still watching?", ""));
    let keep = gtk::Button::with_label("Keep watching");
    keep.add_css_class("suggested-action");
    let stop_now = gtk::Button::with_label("Stop");
    banner.append(&keep);
    banner.append(&stop_now);
    banner.set_visible(false);
    view.root.add_overlay(&banner);

    let frame = widgets::vbox(0);
    frame.add_css_class("window-frame");
    frame.append(&header.root);
    frame.append(&view.root);
    window.set_child(Some(&frame));

    let live_state = Rc::new(Live {
        camera: entity_id.to_string(),
        stream: stream_of(entity_id, &app.home.borrow().entities),
        view: view.clone(),
        toggle: toggle.clone(),
        mute: mute.clone(),
        banner,
        on: Cell::new(false),
        showing: Cell::new(false),
        plugged,
        session: Cell::new(0),
        handlers: RefCell::new(Vec::new()),
        on_refresh: on_refresh.clone(),
        quiet: Cell::new(false),
    });
    {
        let l = live_state.clone();
        toggle.connect_toggled(move |t| {
            if l.quiet.get() {
                return;
            }
            if t.is_active() {
                l.start();
            } else {
                l.stop(None);
            }
        });
        mute.connect_toggled(|m| {
            live::set_muted(!m.is_active());
            m.set_icon_name(if m.is_active() {
                "audio-volume-high-symbolic"
            } else {
                "audio-volume-muted-symbolic"
            });
        });
        let l = live_state.clone();
        keep.connect_clicked(move |_| l.keep_watching());
        let l = live_state.clone();
        stop_now.connect_clicked(move |_| l.stop(None));
        let l = live_state.clone();
        window.connect_close_request(move |_| {
            l.stop(None);
            glib::Propagation::Proceed
        });
    }

    view.attach_layer(entity_id);
    match cached(entity_id) {
        Some(texture) => view.show_texture(&texture),
        None => {
            let (v, note) = (view.clone(), view.note.clone());
            load(entity_id, move |result| match result {
                Ok(texture) => v.show_texture(&texture),
                Err(error) => note.set_text(&error),
            });
        }
    }
    {
        let w = window.clone();
        close.connect_clicked(move |_| w.close());
        let (id, v, l) = (entity_id.to_string(), view.clone(), live_state.clone());
        refresh.connect_clicked(move |button| {
            if l.on.get() {
                return;
            }
            button.set_sensitive(false);
            v.note.set_text("Getting a new picture…");
            v.note.set_visible(true);
            let (v, button, on_refresh) = (v.clone(), button.clone(), on_refresh.clone());
            load(&id, move |result| {
                button.set_sensitive(true);
                match result {
                    Ok(texture) => {
                        v.show_texture(&texture);
                        if let Some(f) = &on_refresh {
                            f(&texture);
                        }
                    }
                    Err(error) => v.note.set_text(&error),
                }
            });
        });
    }
    view.keys(&window);
    let keys = gtk::EventControllerKey::new();
    {
        let (w, toggle, mute) = (window.clone(), toggle.clone(), mute.clone());
        keys.connect_key_pressed(move |_, key, _, _| {
            match key {
                gdk::Key::Escape => w.close(),
                gdk::Key::l if can_stream => toggle.set_active(!toggle.is_active()),
                gdk::Key::m if mute.is_visible() => mute.set_active(!mute.is_active()),
                _ => return glib::Propagation::Proceed,
            }
            glib::Propagation::Stop
        });
    }
    window.add_controller(keys);
    window.present();
    if start_live && can_stream {
        toggle.set_active(true);
    }
}
