use crate::builder::HomeContext;
use gtk::{cairo, prelude::*};
use nixe_home::automation::{
    BUS, DryRunContext, Edge, INTERFACE, Node, NodeKind, Owner, PATH, Point, Workflow,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

const NODE_W: f64 = 190.0;
const NODE_H: f64 = 70.0;
const KINDS: &[&str] = &[
    "Manual trigger",
    "Startup trigger",
    "Interval trigger",
    "Window opened",
    "Window closed",
    "Application running?",
    "Branch on application",
    "Parallel",
    "Delay",
    "Notification",
    "Workspace",
    "Volume",
    "Launch application",
    "Stop",
];

struct CanvasState {
    workflow: Workflow,
    selected: Option<String>,
    connecting: Option<String>,
    pan: (f64, f64),
    zoom: f64,
    undo: Vec<Workflow>,
    redo: Vec<Workflow>,
    dry_results: HashMap<String, String>,
}

fn kind_name(kind: &NodeKind) -> &'static str {
    match kind {
        NodeKind::Manual => "Manual trigger",
        NodeKind::Startup => "Startup trigger",
        NodeKind::Interval { .. } => "Interval trigger",
        NodeKind::WindowOpened { .. } => "Window opened",
        NodeKind::WindowClosed { .. } => "Window closed",
        NodeKind::EntityState { .. } => "Entity state",
        NodeKind::AppRunning { .. } => "Application running?",
        NodeKind::EntityIs { .. } => "Entity is?",
        NodeKind::BranchAppRunning { .. } => "Branch on application",
        NodeKind::Parallel => "Parallel",
        NodeKind::Delay { .. } => "Delay",
        NodeKind::Notify { .. } => "Notification",
        NodeKind::SwitchWorkspace { .. } => "Workspace",
        NodeKind::SetVolume { .. } => "Volume",
        NodeKind::LaunchApplication { .. } => "Launch application",
        NodeKind::SetEntity { .. } => "Set entity",
        NodeKind::ActivateScene { .. } => "Activate scene",
        NodeKind::Stop => "Stop",
    }
}

fn kind_value(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Interval { seconds } | NodeKind::Delay { seconds } => seconds.to_string(),
        NodeKind::WindowOpened { class }
        | NodeKind::WindowClosed { class }
        | NodeKind::AppRunning { class }
        | NodeKind::BranchAppRunning { class } => class.clone(),
        NodeKind::Notify { title, body } => format!("{title} | {body}"),
        NodeKind::SwitchWorkspace { number } => number.to_string(),
        NodeKind::SetVolume { percent } => percent.to_string(),
        NodeKind::LaunchApplication { desktop_id } => desktop_id.clone(),
        NodeKind::EntityState { entity_id, state } | NodeKind::EntityIs { entity_id, state } => {
            format!("{entity_id} | {state}")
        }
        NodeKind::SetEntity { entity_id, turn_on } => {
            format!("{entity_id} | {}", if *turn_on { "on" } else { "off" })
        }
        NodeKind::ActivateScene { entity_id } => entity_id.clone(),
        _ => String::new(),
    }
}

fn make_kind(name: &str, value: &str) -> Result<NodeKind, String> {
    let value = value.trim();
    Ok(match name {
        "Manual trigger" => NodeKind::Manual,
        "Startup trigger" => NodeKind::Startup,
        "Interval trigger" => NodeKind::Interval {
            seconds: value.parse().map_err(|_| "Enter interval seconds")?,
        },
        "Window opened" => NodeKind::WindowOpened {
            class: value.into(),
        },
        "Window closed" => NodeKind::WindowClosed {
            class: value.into(),
        },
        "Application running?" => NodeKind::AppRunning {
            class: value.into(),
        },
        "Branch on application" => NodeKind::BranchAppRunning {
            class: value.into(),
        },
        "Parallel" => NodeKind::Parallel,
        "Delay" => NodeKind::Delay {
            seconds: value.parse().map_err(|_| "Enter delay seconds")?,
        },
        "Notification" => {
            let (title, body) = value.split_once('|').unwrap_or((value, ""));
            NodeKind::Notify {
                title: title.trim().into(),
                body: body.trim().into(),
            }
        }
        "Workspace" => NodeKind::SwitchWorkspace {
            number: value.parse().map_err(|_| "Enter workspace number")?,
        },
        "Volume" => NodeKind::SetVolume {
            percent: value.parse().map_err(|_| "Enter volume percent")?,
        },
        "Launch application" => NodeKind::LaunchApplication {
            desktop_id: value.into(),
        },
        "Stop" => NodeKind::Stop,
        _ => return Err("Select a node type".into()),
    })
}

fn snapshot(state: &mut CanvasState) {
    state.undo.push(state.workflow.clone());
    if state.undo.len() > 50 {
        state.undo.remove(0);
    }
    state.redo.clear();
    state.dry_results.clear();
}

fn layout(workflow: &mut Workflow) {
    let order = workflow
        .validate()
        .unwrap_or_else(|_| workflow.nodes.iter().map(|n| n.id.clone()).collect());
    let mut depth = HashMap::<String, usize>::new();
    let mut rows = HashMap::<usize, usize>::new();
    for id in order {
        let d = workflow
            .edges
            .iter()
            .find(|e| e.to == id)
            .and_then(|e| depth.get(&e.from).copied())
            .map_or(0, |d| d + 1);
        depth.insert(id.clone(), d);
        let row = rows.entry(d).or_default();
        workflow.positions.insert(
            id,
            Point {
                x: 36.0 + d as f64 * 260.0,
                y: 50.0 + *row as f64 * 130.0,
            },
        );
        *row += 1;
    }
}

fn hit(state: &CanvasState, x: f64, y: f64) -> Option<String> {
    let x = (x - state.pan.0) / state.zoom;
    let y = (y - state.pan.1) / state.zoom;
    state
        .workflow
        .nodes
        .iter()
        .rev()
        .find(|n| {
            state
                .workflow
                .positions
                .get(&n.id)
                .is_some_and(|p| x >= p.x && x <= p.x + NODE_W && y >= p.y && y <= p.y + NODE_H)
        })
        .map(|n| n.id.clone())
}

fn draw(state: &CanvasState, cr: &cairo::Context, width: i32, height: i32) {
    cr.set_source_rgb(0.10, 0.11, 0.13);
    let _ = cr.paint();
    let _ = cr.save();
    cr.translate(state.pan.0, state.pan.1);
    cr.scale(state.zoom, state.zoom);
    let x0 = -state.pan.0 / state.zoom;
    let y0 = -state.pan.1 / state.zoom;
    let x1 = x0 + width as f64 / state.zoom;
    let y1 = y0 + height as f64 / state.zoom;
    cr.set_source_rgba(0.6, 0.65, 0.7, 0.12);
    for x in ((x0 / 24.0).floor() as i32)..=((x1 / 24.0).ceil() as i32) {
        for y in ((y0 / 24.0).floor() as i32)..=((y1 / 24.0).ceil() as i32) {
            cr.arc(
                x as f64 * 24.0,
                y as f64 * 24.0,
                1.0,
                0.0,
                std::f64::consts::TAU,
            );
            let _ = cr.fill();
        }
    }
    for edge in &state.workflow.edges {
        let (Some(a), Some(b)) = (
            state.workflow.positions.get(&edge.from),
            state.workflow.positions.get(&edge.to),
        ) else {
            continue;
        };
        let (sx, sy, tx, ty) = (a.x + NODE_W, a.y + NODE_H / 2.0, b.x, b.y + NODE_H / 2.0);
        cr.set_source_rgb(0.38, 0.69, 0.85);
        cr.set_line_width(2.0);
        cr.move_to(sx, sy);
        cr.curve_to(sx + 70.0, sy, tx - 70.0, ty, tx, ty);
        let _ = cr.stroke();
        if let Some(port) = &edge.output {
            cr.move_to(sx + 18.0, sy - 6.0);
            cr.set_font_size(11.0);
            let _ = cr.show_text(port);
        }
    }
    for node in &state.workflow.nodes {
        let Some(p) = state.workflow.positions.get(&node.id) else {
            continue;
        };
        let selected = state.selected.as_deref() == Some(&node.id);
        cr.set_source_rgb(
            if selected { 0.22 } else { 0.16 },
            if selected { 0.31 } else { 0.18 },
            if selected { 0.38 } else { 0.22 },
        );
        cr.rectangle(p.x, p.y, NODE_W, NODE_H);
        let _ = cr.fill_preserve();
        match state.dry_results.get(&node.id).map(String::as_str) {
            Some("skipped") => cr.set_source_rgb(0.40, 0.43, 0.46),
            Some("condition false" | "condition unknown") => cr.set_source_rgb(0.93, 0.69, 0.29),
            Some(_) => cr.set_source_rgb(0.34, 0.78, 0.50),
            None => cr.set_source_rgb(
                if selected { 0.45 } else { 0.31 },
                if selected { 0.76 } else { 0.35 },
                if selected { 0.94 } else { 0.40 },
            ),
        }
        cr.set_line_width(if selected { 2.0 } else { 1.0 });
        let _ = cr.stroke();
        cr.set_source_rgb(0.95, 0.96, 0.97);
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        cr.set_font_size(13.0);
        cr.move_to(p.x + 12.0, p.y + 25.0);
        let _ = cr.show_text(kind_name(&node.kind));
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(11.0);
        let value = kind_value(&node.kind);
        let short: String = value.chars().take(27).collect();
        cr.move_to(p.x + 12.0, p.y + 49.0);
        let _ = cr.show_text(&short);
    }
    let _ = cr.restore();
}

fn inspector(
    state: Rc<RefCell<CanvasState>>,
    panel: gtk::Box,
    area: gtk::DrawingArea,
    feedback: gtk::Label,
) {
    while let Some(child) = panel.first_child() {
        panel.remove(&child);
    }
    let heading = gtk::Label::new(Some("Inspector"));
    heading.set_halign(gtk::Align::Start);
    heading.add_css_class("home-heading");
    panel.append(&heading);
    let Some(id) = state.borrow().selected.clone() else {
        panel.append(&gtk::Label::new(Some(
            "Select a node to edit or connect it.",
        )));
        return;
    };
    let (kind, value, edges) = {
        let s = state.borrow();
        let node = s.workflow.nodes.iter().find(|n| n.id == id).unwrap();
        (
            kind_name(&node.kind),
            kind_value(&node.kind),
            s.workflow
                .edges
                .iter()
                .filter(|e| e.from == id)
                .map(|e| format!("{} → {}", e.output.as_deref().unwrap_or("next"), e.to))
                .collect::<Vec<_>>(),
        )
    };
    let name = gtk::Label::new(Some(&id));
    name.set_halign(gtk::Align::Start);
    name.add_css_class("home-muted");
    panel.append(&name);
    let chooser = gtk::DropDown::from_strings(KINDS);
    if let Some(index) = KINDS.iter().position(|entry| *entry == kind) {
        chooser.set_selected(index as u32);
    }
    chooser.set_sensitive(!matches!(
        kind,
        "Entity state" | "Entity is?" | "Set entity" | "Activate scene"
    ));
    panel.append(&chooser);
    let input = gtk::Entry::new();
    input.set_text(&value);
    input.set_placeholder_text(Some("Value, or title | body"));
    panel.append(&input);
    let apply = gtk::Button::with_label("Apply node changes");
    apply.set_sensitive(KINDS.contains(&kind));
    panel.append(&apply);
    let s = state.clone();
    let a = area.clone();
    let f = feedback.clone();
    let apply_id = id.clone();
    apply.connect_clicked(move |_| {
        let Some(name) = KINDS.get(chooser.selected() as usize) else {
            f.set_text("Choose a node type");
            return;
        };
        match make_kind(name, &input.text()) {
            Ok(kind) => {
                let mut s = s.borrow_mut();
                snapshot(&mut s);
                s.workflow
                    .nodes
                    .iter_mut()
                    .find(|n| n.id == apply_id)
                    .unwrap()
                    .kind = kind;
                f.set_text("Node updated; save to keep changes");
                a.queue_draw();
            }
            Err(e) => f.set_text(&e),
        }
    });
    let links = gtk::Label::new(Some(&format!("Outputs: {}", edges.join(", "))));
    links.set_wrap(true);
    links.set_halign(gtk::Align::Start);
    panel.append(&links);
    for edge in state
        .borrow()
        .workflow
        .edges
        .iter()
        .filter(|e| e.from == id)
        .cloned()
        .collect::<Vec<_>>()
    {
        let disconnect = gtk::Button::with_label(&format!("Remove → {}", edge.to));
        panel.append(&disconnect);
        let s = state.clone();
        let a = area.clone();
        let p = panel.clone();
        let f = feedback.clone();
        disconnect.connect_clicked(move |_| {
            let mut state = s.borrow_mut();
            snapshot(&mut state);
            state
                .workflow
                .edges
                .retain(|e| !(e.from == edge.from && e.to == edge.to));
            drop(state);
            a.queue_draw();
            inspector(s.clone(), p.clone(), a.clone(), f.clone());
        });
    }
    let connect = gtk::Button::with_label("Connect to node…");
    panel.append(&connect);
    let s = state.clone();
    let f = feedback.clone();
    connect.connect_clicked(move |_| {
        let selected = s.borrow().selected.clone();
        s.borrow_mut().connecting = selected;
        f.set_text("Click a target node on the canvas");
    });
    let remove = gtk::Button::with_label("Remove node");
    panel.append(&remove);
    let s = state.clone();
    let a = area.clone();
    let p = panel.clone();
    let f = feedback.clone();
    remove.connect_clicked(move |_| {
        let mut state = s.borrow_mut();
        let Some(id) = state.selected.clone() else {
            return;
        };
        snapshot(&mut state);
        state.workflow.nodes.retain(|n| n.id != id);
        state.workflow.edges.retain(|e| e.from != id && e.to != id);
        state.workflow.positions.remove(&id);
        state.selected = None;
        drop(state);
        f.set_text("Node removed; reconnect any orphaned nodes");
        a.queue_draw();
        inspector(s.clone(), p.clone(), a.clone(), f.clone());
    });
}

pub fn present(
    parent: &gtk::Window,
    mut workflow: Workflow,
    home: HomeContext,
    on_saved: Rc<dyn Fn()>,
) {
    if workflow.positions.len() != workflow.nodes.len() {
        layout(&mut workflow);
    }
    let state = Rc::new(RefCell::new(CanvasState {
        workflow,
        selected: None,
        connecting: None,
        pan: (0.0, 0.0),
        zoom: 1.0,
        undo: vec![],
        redo: vec![],
        dry_results: HashMap::new(),
    }));
    let window = gtk::Window::builder()
        .title("Automation canvas")
        .transient_for(parent)
        .default_width(1100)
        .default_height(690)
        .build();
    window.add_css_class("home-window");
    let root = gtk::Box::new(gtk::Orientation::Vertical, 8);
    root.set_margin_top(10);
    root.set_margin_bottom(10);
    root.set_margin_start(10);
    root.set_margin_end(10);
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let workflow_name = gtk::Entry::new();
    workflow_name.set_text(&state.borrow().workflow.name);
    workflow_name.set_width_chars(24);
    workflow_name.set_placeholder_text(Some("Automation name"));
    toolbar.append(&workflow_name);
    let add = gtk::Button::with_label("Add node");
    let auto = gtk::Button::with_label("Auto layout");
    let undo = gtk::Button::with_label("Undo");
    let redo = gtk::Button::with_label("Redo");
    let dry = gtk::Button::with_label("Dry run");
    let save = gtk::Button::with_label("Save automation");
    save.add_css_class("suggested-action");
    let palette = gtk::DropDown::from_strings(KINDS);
    palette.set_selected(9);
    for widget in [&add, &auto, &undo, &redo, &dry, &save] {
        toolbar.append(widget);
    }
    toolbar.insert_child_after(&palette, Some(&add));
    root.append(&toolbar);
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let area = gtk::DrawingArea::new();
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_content_width(780);
    area.set_content_height(550);
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 9);
    panel.set_width_request(245);
    body.append(&area);
    body.append(&panel);
    root.append(&body);
    let feedback = gtk::Label::new(Some(
        "Drag nodes to move them. Scroll to zoom. Drag empty space to pan.",
    ));
    feedback.set_halign(gtk::Align::Start);
    feedback.set_wrap(true);
    root.append(&feedback);
    window.set_child(Some(&root));
    inspector(state.clone(), panel.clone(), area.clone(), feedback.clone());
    let s = state.clone();
    area.set_draw_func(move |_, cr, w, h| draw(&s.borrow(), cr, w, h));
    let click = gtk::GestureClick::new();
    let s = state.clone();
    let a = area.clone();
    let p = panel.clone();
    let f = feedback.clone();
    click.connect_pressed(move |_, _, x, y| {
        let target = hit(&s.borrow(), x, y);
        let source = s.borrow_mut().connecting.take();
        if let (Some(from), Some(to)) = (source, target.clone())
            && from != to
        {
            let mut state = s.borrow_mut();
            let from_kind = &state
                .workflow
                .nodes
                .iter()
                .find(|n| n.id == from)
                .unwrap()
                .kind;
            let output = if matches!(from_kind, NodeKind::BranchAppRunning { .. }) {
                if state
                    .workflow
                    .edges
                    .iter()
                    .any(|e| e.from == from && e.output.as_deref() == Some("true"))
                {
                    Some("false".into())
                } else {
                    Some("true".into())
                }
            } else {
                None
            };
            snapshot(&mut state);
            state.workflow.edges.retain(|e| e.to != to);
            state.workflow.edges.push(Edge { from, to, output });
            f.set_text("Connection updated; validate before saving");
        }
        s.borrow_mut().selected = target;
        a.queue_draw();
        inspector(s.clone(), p.clone(), a.clone(), f.clone());
    });
    area.add_controller(click);
    let drag = gtk::GestureDrag::new();
    let origin = Rc::new(RefCell::new((
        None::<String>,
        0.0,
        0.0,
        0.0,
        0.0,
        None::<Workflow>,
    )));
    let s = state.clone();
    let o = origin.clone();
    drag.connect_drag_begin(move |_, x, y| {
        let state = s.borrow();
        let id = hit(&state, x, y);
        let point = id
            .as_ref()
            .and_then(|id| state.workflow.positions.get(id))
            .copied()
            .unwrap_or(Point { x: 0.0, y: 0.0 });
        *o.borrow_mut() = (
            id,
            point.x,
            point.y,
            state.pan.0,
            state.pan.1,
            Some(state.workflow.clone()),
        );
    });
    let s = state.clone();
    let o = origin.clone();
    let a = area.clone();
    drag.connect_drag_update(move |_, dx, dy| {
        let origin = o.borrow();
        let mut state = s.borrow_mut();
        if let Some(id) = &origin.0 {
            let zoom = state.zoom;
            if let Some(p) = state.workflow.positions.get_mut(id) {
                p.x = ((origin.1 + dx / zoom) / 12.0).round() * 12.0;
                p.y = ((origin.2 + dy / zoom) / 12.0).round() * 12.0;
            }
        } else {
            state.pan = (origin.3 + dx, origin.4 + dy);
        }
        a.queue_draw();
    });
    let s = state.clone();
    let o = origin.clone();
    drag.connect_drag_end(move |_, dx, dy| {
        if (dx.abs() > 2.0 || dy.abs() > 2.0)
            && o.borrow().0.is_some()
            && let Some(before) = o.borrow_mut().5.take()
        {
            let mut state = s.borrow_mut();
            state.undo.push(before);
            state.redo.clear();
        }
    });
    area.add_controller(drag);
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    let s = state.clone();
    let a = area.clone();
    scroll.connect_scroll(move |_, _, dy| {
        let mut state = s.borrow_mut();
        state.zoom = (state.zoom * if dy > 0.0 { 0.9 } else { 1.1 }).clamp(0.4, 2.5);
        a.queue_draw();
        gtk::glib::Propagation::Stop
    });
    area.add_controller(scroll);
    let s = state.clone();
    let a = area.clone();
    let p = panel.clone();
    let f = feedback.clone();
    add.connect_clicked(move |_| {
        let Some(name) = KINDS.get(palette.selected() as usize) else {
            return;
        };
        let default = match *name {
            "Interval trigger" => "60",
            "Delay" => "1",
            "Workspace" => "1",
            "Volume" => "50",
            "Notification" => "Home | Automation ran",
            "Launch application" => "org.example.App.desktop",
            _ => "",
        };
        let Ok(kind) = make_kind(name, default) else {
            return;
        };
        let mut state = s.borrow_mut();
        snapshot(&mut state);
        let id = format!("node-{}", uuid::Uuid::new_v4().simple());
        let count = state.workflow.nodes.len();
        state.workflow.nodes.push(Node {
            id: id.clone(),
            kind,
        });
        state.workflow.positions.insert(
            id.clone(),
            Point {
                x: 60.0 + (count % 4) as f64 * 230.0,
                y: 80.0 + (count / 4) as f64 * 130.0,
            },
        );
        state.selected = Some(id);
        drop(state);
        f.set_text("Node added; connect it to the workflow");
        a.queue_draw();
        inspector(s.clone(), p.clone(), a.clone(), f.clone());
    });
    let s = state.clone();
    let a = area.clone();
    auto.connect_clicked(move |_| {
        let mut state = s.borrow_mut();
        snapshot(&mut state);
        layout(&mut state.workflow);
        a.queue_draw();
    });
    let s = state.clone();
    let a = area.clone();
    let p = panel.clone();
    let f = feedback.clone();
    undo.connect_clicked(move |_| {
        let mut state = s.borrow_mut();
        if let Some(prev) = state.undo.pop() {
            let current = std::mem::replace(&mut state.workflow, prev);
            state.redo.push(current);
            state.selected = None;
        }
        drop(state);
        a.queue_draw();
        inspector(s.clone(), p.clone(), a.clone(), f.clone());
    });
    let s = state.clone();
    let a = area.clone();
    let p = panel.clone();
    let f = feedback.clone();
    redo.connect_clicked(move |_| {
        let mut state = s.borrow_mut();
        if let Some(next) = state.redo.pop() {
            let current = std::mem::replace(&mut state.workflow, next);
            state.undo.push(current);
            state.selected = None;
        }
        drop(state);
        a.queue_draw();
        inspector(s.clone(), p.clone(), a.clone(), f.clone());
    });
    let s = state.clone();
    let f = feedback.clone();
    let home_dry = home.clone();
    let a = area.clone();
    dry.connect_clicked(move |_| {
        let context = DryRunContext {
            entity_states: home_dry
                .entities
                .iter()
                .map(|(id, _, state)| (id.clone(), state.clone()))
                .collect(),
            running_apps: None,
        };
        let result = s.borrow().workflow.dry_run(&context);
        match result {
            Ok(report) => {
                s.borrow_mut().dry_results = report
                    .steps
                    .iter()
                    .map(|step| (step.node_id.clone(), step.result.clone()))
                    .collect();
                a.queue_draw();
                f.set_text(
                    &report
                        .steps
                        .iter()
                        .map(|step| format!("{}: {}", step.node_id, step.result))
                        .collect::<Vec<_>>()
                        .join(" · "),
                );
            }
            Err(e) => f.set_text(&e),
        }
    });
    let s = state.clone();
    let f = feedback.clone();
    let w = window.clone();
    save.clone().connect_clicked(move |_| {
        let mut workflow = s.borrow().workflow.clone();
        workflow.name = workflow_name.text().to_string();
        if let Err(e) = workflow.validate() {
            f.set_text(&e);
            return;
        }
        save.set_sensitive(false);
        f.set_text("Saving automation…");
        let (tx, rx) = async_channel::bounded(1);
        let home = home.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                if workflow.owner == Owner::HomeAssistant {
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    rt.block_on(crate::ha_automation::save(
                        &home.base,
                        &home.token,
                        &workflow,
                    ))?;
                }
                let conn = zbus::blocking::Connection::session().map_err(|e| e.to_string())?;
                let proxy = zbus::blocking::Proxy::new(&conn, BUS, PATH, INTERFACE)
                    .map_err(|e| e.to_string())?;
                let json = serde_json::to_string(&workflow).map_err(|e| e.to_string())?;
                let _: String = proxy
                    .call("SaveWorkflow", &json)
                    .map_err(|e| e.to_string())?;
                Ok(())
            })();
            let _ = tx.send_blocking(result);
        });
        let f = f.clone();
        let save = save.clone();
        let w = w.clone();
        let on_saved = on_saved.clone();
        gtk::glib::MainContext::default().spawn_local(async move {
            match rx.recv().await {
                Ok(Ok(())) => {
                    w.close();
                    on_saved();
                }
                Ok(Err(e)) => {
                    f.set_text(&e);
                    save.set_sensitive(true);
                }
                Err(_) => {
                    f.set_text("Save failed");
                    save.set_sensitive(true);
                }
            }
        });
    });
    window.present();
}
