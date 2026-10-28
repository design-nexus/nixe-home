//! Home's layer over a robot's map. The integration draws the floor plan;
//! Home draws what you read and touch on top of it, sharp at any zoom: room
//! or zone names (tap one to clean or mow it), the robot, its dock, no-go
//! zones, walls and the mower's trail. Positions come from the map camera's
//! calibration (vacuum) or the mower map's projection.
use crate::{
    app, cards,
    device::Kind,
    ha::{self, Entity},
};
use gtk::{cairo, gdk, glib, prelude::*};
use nyx_ui::widgets;
use serde_json::{Value, json};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

type Point = (f64, f64);

/// What tapping a name does.
#[derive(Clone, Debug, PartialEq)]
pub enum Tap {
    /// Clean one room (segment) of a vacuum.
    Room { vacuum: String, id: u64 },
    /// Mow one saved zone of a mower.
    Zone { mower: String, id: u64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub text: String,
    pub at: Point,
    pub tap: Option<Tap>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marker {
    pub at: Point,
    /// Direction it faces in image space, radians clockwise from +x.
    pub heading: Option<f64>,
}

/// Everything on Home's layer, in the map image's pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layer {
    pub labels: Vec<Label>,
    pub robot: Option<Marker>,
    pub dock: Option<Point>,
    /// No-go and no-mop zones as polygons.
    pub areas: Vec<Vec<Point>>,
    pub walls: Vec<(Point, Point)>,
    pub trail: Vec<Point>,
}

/// Robot coordinates to image pixels, from three calibration points.
#[derive(Clone, Copy, Debug)]
pub struct Affine {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl Affine {
    /// From `[{"vacuum": {x, y}, "map": {x, y}}, …]`.
    pub fn from_calibration(points: &Value) -> Option<Self> {
        let pts: Vec<(Point, Point)> = points
            .as_array()?
            .iter()
            .filter_map(|p| {
                Some((
                    (p["vacuum"]["x"].as_f64()?, p["vacuum"]["y"].as_f64()?),
                    (p["map"]["x"].as_f64()?, p["map"]["y"].as_f64()?),
                ))
            })
            .collect();
        if pts.len() < 3 {
            return None;
        }
        let [(v0, m0), (v1, m1), (v2, m2)] = [pts[0], pts[1], pts[2]];
        // Solve m = A·v + t for both rows with Cramer's rule.
        let det = v0.0 * (v1.1 - v2.1) - v0.1 * (v1.0 - v2.0) + (v1.0 * v2.1 - v2.0 * v1.1);
        if det.abs() < 1e-9 {
            return None;
        }
        let solve = |r0: f64, r1: f64, r2: f64| {
            let a = (r0 * (v1.1 - v2.1) - v0.1 * (r1 - r2) + (r1 * v2.1 - r2 * v1.1)) / det;
            let b = (v0.0 * (r1 - r2) - r0 * (v1.0 - v2.0) + (v1.0 * r2 - v2.0 * r1)) / det;
            let c = (v0.0 * (v1.1 * r2 - v2.1 * r1) - v0.1 * (v1.0 * r2 - v2.0 * r1)
                + r0 * (v1.0 * v2.1 - v2.0 * v1.1))
                / det;
            (a, b, c)
        };
        let (a, b, c) = solve(m0.0, m1.0, m2.0);
        let (d, e, f) = solve(m0.1, m1.1, m2.1);
        Some(Self { a, b, c, d, e, f })
    }

    pub fn apply(&self, (x, y): Point) -> Point {
        (
            self.a * x + self.b * y + self.c,
            self.d * x + self.e * y + self.f,
        )
    }

    /// A robot-frame angle (degrees) as an image direction (radians).
    fn heading(&self, degrees: f64) -> f64 {
        let r = degrees.to_radians();
        let (dx, dy) = (r.cos(), r.sin());
        (self.d * dx + self.e * dy).atan2(self.a * dx + self.b * dy)
    }
}

fn point(v: &Value) -> Option<Point> {
    Some((v["x"].as_f64()?, v["y"].as_f64()?))
}

/// Corners of a zone given as `{x0, y0, …, x3, y3}`, `{x0, y0, x1, y1}` (a
/// rectangle) or a flat list of numbers.
fn corners(v: &Value) -> Vec<Point> {
    let nums: Vec<f64> = match v {
        Value::Array(list) => list.iter().filter_map(Value::as_f64).collect(),
        Value::Object(map) => (0..4)
            .flat_map(|i| [format!("x{i}"), format!("y{i}")])
            .map_while(|k| map.get(&k).and_then(Value::as_f64))
            .collect(),
        _ => Vec::new(),
    };
    match nums.len() {
        4 => vec![
            (nums[0], nums[1]),
            (nums[2], nums[1]),
            (nums[2], nums[3]),
            (nums[0], nums[3]),
        ],
        n if n >= 6 => nums.chunks(2).map(|c| (c[0], c[1])).collect(),
        _ => Vec::new(),
    }
}

/// The vacuum's layer from its map camera's attributes.
pub fn vacuum_layer(attrs: &Value, vacuum: &str) -> Option<Layer> {
    let t = Affine::from_calibration(&attrs["calibration_points"])?;
    let mut layer = Layer::default();
    if let Some(rooms) = attrs["rooms"].as_object() {
        for room in rooms.values() {
            if room["visibility"].as_str() == Some("Hidden") {
                continue;
            }
            let (Some(name), Some(at)) = (room["name"].as_str(), point(room)) else {
                continue;
            };
            layer.labels.push(Label {
                text: name.to_string(),
                at: t.apply(at),
                tap: room["room_id"].as_u64().map(|id| Tap::Room {
                    vacuum: vacuum.to_string(),
                    id,
                }),
            });
        }
    }
    layer.robot = point(&attrs["vacuum_position"]).map(|at| Marker {
        at: t.apply(at),
        heading: attrs["vacuum_position"]["a"].as_f64().map(|a| t.heading(a)),
    });
    layer.dock = point(&attrs["charger_position"]).map(|at| t.apply(at));
    for key in ["no_go_areas", "no_mopping_areas"] {
        for area in attrs[key].as_array().into_iter().flatten() {
            let poly: Vec<Point> = corners(area).into_iter().map(|p| t.apply(p)).collect();
            if poly.len() >= 3 {
                layer.areas.push(poly);
            }
        }
    }
    for wall in attrs["virtual_walls"].as_array().into_iter().flatten() {
        let c = corners(wall);
        // A wall is two points: corners() makes a rectangle of four.
        if c.len() == 4 {
            layer.walls.push((t.apply(c[0]), t.apply(c[2])));
        }
    }
    Some(layer)
}

/// Mower map units to image pixels with the map's projection.
fn project(p: &Value, (x, y): Point) -> Option<Point> {
    let scale = p["scale"].as_f64()?;
    let pad = p["padding"].as_f64().unwrap_or(0.0);
    let px = (p["max_x"].as_f64()? - x) * scale + pad;
    let py = (y - p["min_y"].as_f64()?) * scale + pad;
    let (w, h) = (
        p["native_width"].as_f64().unwrap_or(0.0),
        p["native_height"].as_f64().unwrap_or(0.0),
    );
    Some(match p["rotation"].as_i64().unwrap_or(0) {
        90 => (h - py, px),
        180 => (w - px, h - py),
        270 => (py, w - px),
        _ => (px, py),
    })
}

/// The mower's layer from its map data and its map camera's attributes.
pub fn mower_layer(map: &Value, attrs: &Value, mower: &str) -> Option<Layer> {
    let p = &map["projection"];
    let mut layer = Layer::default();
    for zone in map["zones"].as_array().into_iter().flatten() {
        let (Some(x), Some(y)) = (zone["label_x"].as_f64(), zone["label_y"].as_f64()) else {
            continue;
        };
        let name = zone["name"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("Zone {}", zone["zone_id"]));
        layer.labels.push(Label {
            text: name,
            at: project(p, (x, y))?,
            tap: zone["zone_id"].as_u64().map(|id| Tap::Zone {
                mower: mower.to_string(),
                id,
            }),
        });
    }
    let overlay = &map["overlay"];
    if let (Some(x), Some(y)) = (attrs["charger_x"].as_f64(), attrs["charger_y"].as_f64()) {
        layer.dock = project(p, (x, y));
    }
    if overlay["docked"] != true {
        layer.robot = point(&overlay["position"]).map(|at| Marker {
            at,
            heading: overlay["position"]["heading"].as_f64().map(f64::to_radians),
        });
    }
    layer.trail = overlay["trail"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pt| match pt {
            Value::Array(xy) => Some((xy.first()?.as_f64()?, xy.get(1)?.as_f64()?)),
            other => point(other),
        })
        .collect();
    Some(layer)
}

// ---------- On screen ----------

/// The layer over a picture in the viewer: names as buttons in a `Fixed`,
/// shapes on a drawing area, both following the viewer's zoom.
pub struct View {
    fixed: gtk::Fixed,
    area: gtk::DrawingArea,
    layer: Rc<RefCell<Layer>>,
    scale: Rc<Cell<f64>>,
    names: RefCell<Vec<(gtk::Widget, Point)>>,
}

fn rgba(name: &str, fallback: &str) -> gdk::RGBA {
    let hex = nyx_theme::swatches()
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, c)| c)
        .unwrap_or_else(|| fallback.to_string());
    gdk::RGBA::parse(&hex).unwrap_or(gdk::RGBA::WHITE)
}

fn set(cr: &cairo::Context, c: &gdk::RGBA, alpha: f64) {
    cr.set_source_rgba(
        c.red() as f64,
        c.green() as f64,
        c.blue() as f64,
        c.alpha() as f64 * alpha,
    );
}

fn draw(cr: &cairo::Context, layer: &Layer, s: f64) {
    let robot = rgba("violet", "#9d7cd8");
    let danger = rgba("red", "#f7768e");
    let accent = gdk::RGBA::parse(nyx_theme::palette().accent).unwrap_or(gdk::RGBA::WHITE);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_cap(cairo::LineCap::Round);
    for poly in &layer.areas {
        cr.new_path();
        for (i, (x, y)) in poly.iter().enumerate() {
            if i == 0 {
                cr.move_to(x * s, y * s);
            } else {
                cr.line_to(x * s, y * s);
            }
        }
        cr.close_path();
        set(cr, &danger, 0.22);
        let _ = cr.fill_preserve();
        set(cr, &danger, 0.9);
        cr.set_line_width(2.0);
        let _ = cr.stroke();
    }
    for ((x0, y0), (x1, y1)) in &layer.walls {
        cr.move_to(x0 * s, y0 * s);
        cr.line_to(x1 * s, y1 * s);
        set(cr, &danger, 0.95);
        cr.set_line_width(3.5);
        let _ = cr.stroke();
    }
    if layer.trail.len() > 1 {
        for (i, (x, y)) in layer.trail.iter().enumerate() {
            if i == 0 {
                cr.move_to(x * s, y * s);
            } else {
                cr.line_to(x * s, y * s);
            }
        }
        set(cr, &accent, 0.75);
        cr.set_line_width(2.5);
        let _ = cr.stroke();
    }
    if let Some((x, y)) = layer.dock {
        let (x, y) = (x * s, y * s);
        cr.new_path();
        cr.arc(x, y, 9.0, 0.0, std::f64::consts::TAU);
        cr.set_source_rgba(0.1, 0.1, 0.12, 0.85);
        let _ = cr.fill_preserve();
        set(cr, &robot, 1.0);
        cr.set_line_width(2.0);
        let _ = cr.stroke();
        // A small house.
        cr.move_to(x - 4.0, y + 3.5);
        cr.line_to(x - 4.0, y - 1.0);
        cr.line_to(x, y - 4.5);
        cr.line_to(x + 4.0, y - 1.0);
        cr.line_to(x + 4.0, y + 3.5);
        cr.close_path();
        let _ = cr.fill();
    }
    if let Some(m) = layer.robot {
        let (x, y) = (m.at.0 * s, m.at.1 * s);
        if let Some(h) = m.heading {
            cr.move_to(x + 15.0 * h.cos(), y + 15.0 * h.sin());
            cr.line_to(x + 8.0 * (h + 0.6).cos(), y + 8.0 * (h + 0.6).sin());
            cr.line_to(x + 8.0 * (h - 0.6).cos(), y + 8.0 * (h - 0.6).sin());
            cr.close_path();
            set(cr, &robot, 1.0);
            let _ = cr.fill();
        }
        cr.new_path();
        cr.arc(x, y, 9.5, 0.0, std::f64::consts::TAU);
        set(cr, &robot, 1.0);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(1.0, 1.0, 1.0, 0.95);
        cr.set_line_width(2.5);
        let _ = cr.stroke();
    }
}

impl View {
    fn new(stage: &gtk::Overlay) -> Rc<Self> {
        let area = gtk::DrawingArea::new();
        area.set_can_target(false);
        let fixed = gtk::Fixed::new();
        fixed.add_css_class("map-layer");
        stage.add_overlay(&area);
        stage.add_overlay(&fixed);
        let view = Rc::new(Self {
            fixed,
            area,
            layer: Rc::new(RefCell::new(Layer::default())),
            scale: Rc::new(Cell::new(1.0)),
            names: RefCell::new(Vec::new()),
        });
        let (layer, scale) = (view.layer.clone(), view.scale.clone());
        view.area
            .set_draw_func(move |_, cr, _, _| draw(cr, &layer.borrow(), scale.get()));
        view
    }

    /// Follow the viewer's zoom.
    pub fn set_scale(&self, s: f64) {
        self.scale.set(s);
        // Far out, names shrink so they don't cover each other.
        if s < 0.35 {
            self.fixed.add_css_class("small");
        } else {
            self.fixed.remove_css_class("small");
        }
        for (w, (x, y)) in self.names.borrow().iter() {
            let (_, nw, _, _) = w.measure(gtk::Orientation::Horizontal, -1);
            let (_, nh, _, _) = w.measure(gtk::Orientation::Vertical, nw);
            self.fixed.move_(
                w,
                (x * s - nw as f64 / 2.0).max(0.0),
                (y * s - nh as f64 / 2.0).max(0.0),
            );
        }
        self.area.queue_draw();
    }

    fn show(&self, layer: Layer) {
        if *self.layer.borrow() == layer {
            return;
        }
        for (w, _) in self.names.borrow_mut().drain(..) {
            self.fixed.remove(&w);
        }
        for label in &layer.labels {
            let widget: gtk::Widget = match &label.tap {
                Some(tap) => {
                    let b = gtk::Button::with_label(&label.text);
                    b.add_css_class("map-label");
                    b.add_css_class("tappable");
                    let what = match tap {
                        Tap::Room { .. } => format!("Clean {}", label.text),
                        Tap::Zone { .. } => format!("Mow {}", label.text),
                    };
                    b.set_tooltip_text(Some(&what));
                    b.update_property(&[gtk::accessible::Property::Label(&what)]);
                    let (tap, name) = (tap.clone(), label.text.clone());
                    b.connect_clicked(move |b| confirm(b, &tap, &name));
                    b.upcast()
                }
                None => widgets::label(&label.text, "map-label").upcast(),
            };
            self.fixed.put(&widget, 0.0, 0.0);
            self.names.borrow_mut().push((widget, label.at));
        }
        *self.layer.borrow_mut() = layer;
        self.set_scale(self.scale.get());
    }
}

/// Ask before cleaning a room or mowing a zone.
fn confirm(from: &gtk::Button, tap: &Tap, name: &str) {
    match tap.clone() {
        Tap::Room { vacuum, id } => {
            app::toast_action(&format!("Clean {name}?"), "Clean", move || {
                send(
                    "dreame_vacuum",
                    "vacuum_clean_segment",
                    json!({"entity_id": vacuum, "segments": [id]}),
                );
            });
        }
        Tap::Zone { mower, id } => {
            let (dialog, card) = widgets::dialog(&format!("Mow {name}?"), 380);
            if let Some(parent) = from.root().and_downcast::<gtk::Window>() {
                dialog.set_transient_for(Some(&parent));
            }
            let note = widgets::label(
                "Check that the lawn is clear of people, pets and objects, and that the mower's safety features are on.",
                "dim",
            );
            note.set_wrap(true);
            card.append(&note);
            let buttons = widgets::hbox(8);
            buttons.set_halign(gtk::Align::End);
            let cancel = gtk::Button::with_label("Cancel");
            let start = gtk::Button::with_label(&format!("Mow {name}"));
            start.add_css_class("suggested-action");
            buttons.append(&cancel);
            buttons.append(&start);
            card.append(&buttons);
            let d = dialog.clone();
            cancel.connect_clicked(move |_| d.close());
            let d = dialog.clone();
            start.connect_clicked(move |_| {
                send(
                    "dreame_lawn_mower",
                    "start_zone_mowing",
                    json!({"entity_id": mower, "zone_ids": [id]}),
                );
                d.close();
            });
            dialog.present();
        }
    }
}

fn send(domain: &'static str, service: &'static str, data: Value) {
    app::ha_task(
        move |base, token| async move { ha::call_service(&base, &token, domain, service, data).await },
        |()| app::toast("Started."),
    );
}

/// The robot whose map `camera` is, and its main entity.
fn robot_of(camera: &str) -> Option<Entity> {
    let app = app::get()?;
    let home = app.home.borrow();
    home.device_list(true)
        .into_iter()
        .find(|d| d.kind == Kind::Robot && d.entities.iter().any(|e| e == camera))
        .and_then(|d| d.primary(&home.entities).cloned())
}

/// Put Home's layer over `stage` (the viewer's picture) when `camera` is a
/// robot's map, and keep it current.
pub fn attach(camera: &str, stage: &gtk::Overlay) -> Option<Rc<View>> {
    let robot = robot_of(camera)?;
    let view = View::new(stage);
    let (camera, robot_id) = (camera.to_string(), robot.entity_id.clone());
    match robot.domain() {
        "vacuum" => {
            let v = Rc::downgrade(&view);
            let cam = camera.clone();
            let robot = robot_id.clone();
            cards::watch(&view.fixed, vec![camera, robot_id], move |home| {
                if let (Some(view), Some(e)) = (v.upgrade(), home.entities.get(&cam))
                    && let Some(layer) = vacuum_layer(&e.attributes, &robot)
                {
                    view.show(layer);
                }
            });
        }
        "lawn_mower" => {
            let v = Rc::downgrade(&view);
            let (cam, robot) = (camera.clone(), robot_id.clone());
            let debounce = cards::Debounce::default();
            cards::watch(&view.fixed, vec![camera, robot_id], move |home| {
                let Some(attrs) = home.entities.get(&cam).map(|e| e.attributes.clone()) else {
                    return;
                };
                let Some(path) = attrs["mowing_map_api_path"].as_str().map(str::to_string) else {
                    return;
                };
                let (v, robot) = (v.clone(), robot.clone());
                debounce.run(800, move || {
                    app::ha_task(
                        move |base, token| async move {
                            ha::fetch_mowing_map(&base, &token, &path).await
                        },
                        move |map| {
                            if let (Some(view), Some(layer)) =
                                (v.upgrade(), mower_layer(&map, &attrs, &robot))
                            {
                                view.show(layer);
                            }
                        },
                    );
                });
            });
        }
        _ => return None,
    }
    // Fill in from the current state.
    if let Some(app) = app::get() {
        let home = app.home.borrow();
        cards::notify(
            &home,
            Some(&[robot.entity_id.clone()].into_iter().collect()),
        );
    }
    glib::idle_add_local_once({
        let v = Rc::downgrade(&view);
        move || {
            if let Some(v) = v.upgrade() {
                v.set_scale(v.scale.get());
            }
        }
    });
    Some(view)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vacuum_positions_follow_the_calibration() {
        // The Z60's own calibration: x mirrored, 0.08 px per mm.
        let attrs = json!({
            "calibration_points": [
                {"vacuum": {"x": 0, "y": 0}, "map": {"x": 698, "y": 586}},
                {"vacuum": {"x": 1000, "y": 0}, "map": {"x": 618, "y": 586}},
                {"vacuum": {"x": 0, "y": 1000}, "map": {"x": 698, "y": 666}}
            ],
            "rooms": {
                "1": {"room_id": 1, "name": "Bathroom", "x": 3625, "y": 7125, "visibility": "Visible"},
                "2": {"room_id": 2, "name": "Closet", "x": 0, "y": 0, "visibility": "Hidden"}
            },
            "charger_position": {"x": 7730, "y": 2812, "a": 179},
            "vacuum_position": {"x": 7730, "y": 2812, "a": 0},
            "no_go_areas": [{"x0": 0, "y0": 0, "x1": 1000, "y1": 1000}],
            "virtual_walls": [[0, 0, 1000, 0]]
        });
        let layer = vacuum_layer(&attrs, "vacuum.z60").unwrap();
        assert_eq!(layer.labels.len(), 1);
        let room = &layer.labels[0];
        assert_eq!(room.text, "Bathroom");
        assert!((room.at.0 - 408.0).abs() < 0.01 && (room.at.1 - 1156.0).abs() < 0.01);
        assert_eq!(
            room.tap,
            Some(Tap::Room {
                vacuum: "vacuum.z60".into(),
                id: 1
            })
        );
        let dock = layer.dock.unwrap();
        assert!((dock.0 - 79.6).abs() < 0.01 && (dock.1 - 810.96).abs() < 0.01);
        // Facing +x in the robot's frame faces left on the mirrored image.
        let heading = layer.robot.unwrap().heading.unwrap();
        assert!((heading.abs() - std::f64::consts::PI).abs() < 1e-6);
        assert_eq!(layer.areas[0].len(), 4);
        assert_eq!(layer.walls.len(), 1);
    }

    #[test]
    fn mower_zones_use_the_map_projection() {
        let map = json!({
            "projection": {"max_x": 22460, "min_y": -19800, "scale": 0.05090532850491464,
                           "padding": 40, "native_width": 1329, "native_height": 2048, "rotation": 0},
            "zones": [{"zone_id": 1, "name": "Zone1", "label_x": 5133, "label_y": -13684}],
            "overlay": {"position": {"x": 1116, "y": 1048, "heading": null}, "docked": true,
                        "trail": [[10, 20], {"x": 30, "y": 40}]}
        });
        let attrs = json!({"charger_x": 1330, "charger_y": 0});
        let layer = mower_layer(&map, &attrs, "lawn_mower.lidax").unwrap();
        let zone = &layer.labels[0];
        assert!((zone.at.0 - 922.0).abs() < 1.0 && (zone.at.1 - 351.3).abs() < 1.0);
        assert_eq!(
            zone.tap,
            Some(Tap::Zone {
                mower: "lawn_mower.lidax".into(),
                id: 1
            })
        );
        let dock = layer.dock.unwrap();
        assert!((dock.0 - 1115.6).abs() < 1.0 && (dock.1 - 1047.9).abs() < 1.0);
        // Docked: the dock marker stands for the robot.
        assert!(layer.robot.is_none());
        assert_eq!(layer.trail, [(10.0, 20.0), (30.0, 40.0)]);
    }
}
