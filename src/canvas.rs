//! The automation canvas: a graph editor for desktop, Home Assistant and
//! hybrid workflows, with branches, parallel paths, waits and retries.
use crate::{
    builder::{HomeContext, Picker},
    present, rt, service,
};
use gtk::{cairo, gdk, glib, prelude::*};
use nyx_home::automation::{
    ActionPolicy, DryRunContext, Edge, Group, Node, NodeKind, Owner, Point, Workflow,
};
use nyx_theme::Rgb;
use nyx_ui::widgets;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Duration,
};

const NODE_W: f64 = 200.0;
const NODE_H: f64 = 64.0;
const GRID: f64 = 12.0;
const KINDS: &[&str] = &[
    "Manual trigger",
    "Startup trigger",
    "Interval trigger",
    "Window opened",
    "Window closed",
    "Entity state",
    "Application running?",
    "Entity is?",
    "Variable equals?",
    "Branch on application",
    "Parallel",
    "Repeat",
    "Delay",
    "Wait for window",
    "Wait for entity",
    "Notification",
    "Workspace",
    "Volume",
    "Launch application",
    "Turn entity on/off",
    "Activate scene",
    "Set variable",
    "Stop",
];

/// The node types that can run for `owner`.
fn kinds_for(owner: Owner) -> Vec<&'static str> {
    let home = [
        "Entity state",
        "Entity is?",
        "Wait for entity",
        "Turn entity on/off",
        "Activate scene",
    ];
    KINDS
        .iter()
        .copied()
        .filter(|kind| match owner {
            Owner::Desktop => !home.contains(kind),
            Owner::HybridDesktop => true,
            Owner::HomeAssistant => {
                matches!(
                    *kind,
                    "Entity state" | "Entity is?" | "Turn entity on/off" | "Activate scene"
                )
            }
        })
        .collect()
}

#[derive(Clone)]
struct Clipboard {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
    positions: HashMap<String, Point>,
    policies: HashMap<String, ActionPolicy>,
}

struct CanvasState {
    workflow: Workflow,
    selected: Option<String>,
    selection: HashSet<String>,
    clipboard: Option<Clipboard>,
    connecting: Option<(String, Option<String>)>,
    pan: (f64, f64),
    zoom: f64,
    pointer: (f64, f64),
    undo: Vec<Workflow>,
    redo: Vec<Workflow>,
    dry_results: HashMap<String, String>,
    live_details: HashMap<String, String>,
    home: HomeContext,
}

impl CanvasState {
    fn label(&self, kind: &NodeKind) -> String {
        present::node_label(kind, &|id| self.home.name_of(id))
    }
    fn node_label(&self, id: &str) -> String {
        self.workflow
            .nodes
            .iter()
            .find(|n| n.id == id)
            .map_or_else(|| "a removed step".into(), |n| self.label(&n.kind))
    }
}

fn delete_selection(state: &mut CanvasState) {
    if state.selection.is_empty() {
        return;
    }
    snapshot(state);
    let gone = state.selection.clone();
    state.workflow.nodes.retain(|n| !gone.contains(&n.id));
    state
        .workflow
        .edges
        .retain(|e| !gone.contains(&e.from) && !gone.contains(&e.to));
    state.workflow.positions.retain(|id, _| !gone.contains(id));
    state.workflow.policies.retain(|id, _| !gone.contains(id));
    for group in &mut state.workflow.groups {
        group.members.retain(|id| !gone.contains(id));
    }
    state
        .workflow
        .groups
        .retain(|group| group.members.len() >= 2);
    state.selection.clear();
    state.selected = None;
    state.connecting = None;
}

fn copy_selection(state: &mut CanvasState) {
    let ids: HashSet<_> = state
        .selection
        .iter()
        .filter(|id| {
            state
                .workflow
                .nodes
                .iter()
                .any(|n| n.id == **id && !n.kind.is_trigger())
        })
        .cloned()
        .collect();
    if ids.is_empty() {
        return;
    }
    state.clipboard = Some(Clipboard {
        nodes: state
            .workflow
            .nodes
            .iter()
            .filter(|n| ids.contains(&n.id))
            .cloned()
            .collect(),
        edges: state
            .workflow
            .edges
            .iter()
            .filter(|e| ids.contains(&e.from) && ids.contains(&e.to))
            .cloned()
            .collect(),
        positions: state
            .workflow
            .positions
            .iter()
            .filter(|(id, _)| ids.contains(*id))
            .map(|(id, p)| (id.clone(), *p))
            .collect(),
        policies: state
            .workflow
            .policies
            .iter()
            .filter(|(id, _)| ids.contains(*id))
            .map(|(id, p)| (id.clone(), p.clone()))
            .collect(),
    });
}

fn paste_selection(state: &mut CanvasState) -> bool {
    let Some(Clipboard {
        nodes,
        edges,
        positions,
        policies,
    }) = state.clipboard.clone()
    else {
        return false;
    };
    if state.workflow.nodes.len() + nodes.len() > 64 {
        return false;
    }
    snapshot(state);
    let ids: HashMap<String, String> = nodes
        .iter()
        .map(|n| {
            (
                n.id.clone(),
                format!("node-{}", uuid::Uuid::new_v4().simple()),
            )
        })
        .collect();
    state.selection.clear();
    for mut node in nodes {
        let old_id = node.id.clone();
        node.id = ids[&old_id].clone();
        state.selection.insert(node.id.clone());
        state.selected = Some(node.id.clone());
        if let Some(p) = positions.get(&old_id) {
            state.workflow.positions.insert(
                node.id.clone(),
                Point {
                    x: p.x + 48.0,
                    y: p.y + 48.0,
                },
            );
        }
        if let Some(policy) = policies.get(&old_id) {
            state
                .workflow
                .policies
                .insert(node.id.clone(), policy.clone());
        }
        state.workflow.nodes.push(node);
    }
    for mut edge in edges {
        edge.from = ids[&edge.from].clone();
        edge.to = ids[&edge.to].clone();
        state.workflow.edges.push(edge);
    }
    true
}

fn default_value(name: &str) -> &'static str {
    match name {
        "Interval trigger" => "900",
        "Delay" => "5",
        "Workspace" => "1",
        "Volume" => "50",
        "Notification" => "Home | Automation ran",
        "Launch application" => "org.gnome.Nautilus.desktop",
        "Set variable" | "Variable equals?" => "mode | on",
        "Repeat" => "2",
        "Wait for window" => "firefox | 60",
        "Wait for entity" => "binary_sensor.example | on | 60",
        "Entity state" | "Entity is?" => "binary_sensor.example | on",
        "Turn entity on/off" => "light.example | on",
        "Activate scene" => "scene.example",
        "Window opened" | "Window closed" | "Application running?" | "Branch on application" => {
            "firefox"
        }
        _ => "",
    }
}

fn add_node(state: &mut CanvasState, name: &str, point: Point) -> bool {
    if state.workflow.nodes.len() >= 64 {
        return false;
    }
    let Ok(kind) = make_kind(name, default_value(name)) else {
        return false;
    };
    snapshot(state);
    let id = format!("node-{}", uuid::Uuid::new_v4().simple());
    state.workflow.nodes.push(Node {
        id: id.clone(),
        kind,
    });
    state.workflow.positions.insert(
        id.clone(),
        Point {
            x: snap(point.x),
            y: snap(point.y),
        },
    );
    state.selection.clear();
    state.selection.insert(id.clone());
    state.selected = Some(id);
    true
}

fn snap(value: f64) -> f64 {
    (value / GRID).round() * GRID
}

fn group_bounds(workflow: &Workflow, members: &[String]) -> Option<(f64, f64, f64, f64)> {
    let points: Vec<_> = members
        .iter()
        .filter_map(|id| workflow.positions.get(id))
        .collect();
    if points.is_empty() {
        return None;
    }
    let x0 = points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min) - 18.0;
    let y0 = points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min) - 30.0;
    let x1 = points
        .iter()
        .map(|p| p.x + NODE_W)
        .fold(f64::NEG_INFINITY, f64::max)
        + 18.0;
    let y1 = points
        .iter()
        .map(|p| p.y + NODE_H)
        .fold(f64::NEG_INFINITY, f64::max)
        + 18.0;
    Some((x0, y0, x1, y1))
}

fn collapsed_group<'a>(workflow: &'a Workflow, id: &str) -> Option<&'a Group> {
    workflow
        .groups
        .iter()
        .find(|g| g.collapsed && g.members.iter().any(|m| m == id))
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
        NodeKind::Repeat { .. } => "Repeat",
        NodeKind::WaitWindowOpened { .. } => "Wait for window",
        NodeKind::WaitEntityState { .. } => "Wait for entity",
        NodeKind::Notify { .. } => "Notification",
        NodeKind::SwitchWorkspace { .. } => "Workspace",
        NodeKind::SetVolume { .. } => "Volume",
        NodeKind::LaunchApplication { .. } => "Launch application",
        NodeKind::SetEntity { .. } => "Turn entity on/off",
        NodeKind::ActivateScene { .. } => "Activate scene",
        NodeKind::Stop => "Stop",
        NodeKind::SetVariable { .. } => "Set variable",
        NodeKind::VariableEquals { .. } => "Variable equals?",
    }
}

/// A node's parameters as `a | b | c`, the order [`make_kind`] reads them in.
fn kind_value(kind: &NodeKind) -> String {
    match kind {
        NodeKind::Interval { seconds } | NodeKind::Delay { seconds } => seconds.to_string(),
        NodeKind::Repeat { times } => times.to_string(),
        NodeKind::WaitWindowOpened {
            class,
            timeout_seconds,
        } => format!("{class} | {timeout_seconds}"),
        NodeKind::WaitEntityState {
            entity_id,
            state,
            timeout_seconds,
        } => {
            format!("{entity_id} | {state} | {timeout_seconds}")
        }
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
        NodeKind::SetVariable { name, value } | NodeKind::VariableEquals { name, value } => {
            format!("{name} | {value}")
        }
        _ => String::new(),
    }
}

fn number<T: std::str::FromStr>(text: &str, what: &str) -> Result<T, String> {
    text.parse().map_err(|_| format!("Enter {what}"))
}

fn make_kind(name: &str, value: &str) -> Result<NodeKind, String> {
    let value = value.trim();
    let mut parts = value.split('|').map(str::trim);
    let mut next = || parts.next().unwrap_or("");
    Ok(match name {
        "Manual trigger" => NodeKind::Manual,
        "Startup trigger" => NodeKind::Startup,
        "Interval trigger" => NodeKind::Interval {
            seconds: number(next(), "how often it runs")?,
        },
        "Window opened" => NodeKind::WindowOpened {
            class: next().into(),
        },
        "Window closed" => NodeKind::WindowClosed {
            class: next().into(),
        },
        "Application running?" => NodeKind::AppRunning {
            class: next().into(),
        },
        "Branch on application" => NodeKind::BranchAppRunning {
            class: next().into(),
        },
        "Parallel" => NodeKind::Parallel,
        "Delay" => NodeKind::Delay {
            seconds: number(next(), "how long to wait")?,
        },
        "Repeat" => NodeKind::Repeat {
            times: number(next(), "how many times")?,
        },
        "Wait for window" => NodeKind::WaitWindowOpened {
            class: next().into(),
            timeout_seconds: number(next(), "how long to wait")?,
        },
        "Wait for entity" => NodeKind::WaitEntityState {
            entity_id: next().into(),
            state: next().into(),
            timeout_seconds: number(next(), "how long to wait")?,
        },
        "Notification" => {
            let (title, body) = value.split_once('|').unwrap_or((value, ""));
            NodeKind::Notify {
                title: title.trim().into(),
                body: body.trim().into(),
            }
        }
        "Workspace" => NodeKind::SwitchWorkspace {
            number: number(next(), "a workspace number")?,
        },
        "Volume" => NodeKind::SetVolume {
            percent: number(next(), "a volume percent")?,
        },
        "Launch application" => NodeKind::LaunchApplication {
            desktop_id: next().into(),
        },
        "Entity state" => NodeKind::EntityState {
            entity_id: next().into(),
            state: next().into(),
        },
        "Entity is?" => NodeKind::EntityIs {
            entity_id: next().into(),
            state: next().into(),
        },
        "Turn entity on/off" => NodeKind::SetEntity {
            entity_id: next().into(),
            turn_on: next() != "off",
        },
        "Activate scene" => NodeKind::ActivateScene {
            entity_id: next().into(),
        },
        "Set variable" | "Variable equals?" => {
            let variable = next().to_string();
            let contents = next().to_string();
            if variable.is_empty() {
                return Err("Enter a variable name".into());
            }
            if name == "Variable equals?" {
                NodeKind::VariableEquals {
                    name: variable,
                    value: contents,
                }
            } else {
                NodeKind::SetVariable {
                    name: variable,
                    value: contents,
                }
            }
        }
        "Stop" => NodeKind::Stop,
        _ => return Err("Choose a step type".into()),
    })
}

/// How a node's parameters are edited.
#[derive(Clone, Copy)]
enum Field {
    Text(&'static str),
    Number(&'static str, f64, f64),
    Entity(&'static str, fn(&str) -> bool),
    App,
    OnOff,
}

fn any_entity(_: &str) -> bool {
    true
}
fn switchable(id: &str) -> bool {
    ["light.", "switch.", "fan.", "input_boolean."]
        .iter()
        .any(|p| id.starts_with(p))
}
fn scene(id: &str) -> bool {
    id.starts_with("scene.")
}

fn fields(name: &str) -> Vec<Field> {
    use Field::*;
    match name {
        "Interval trigger" => vec![Number("Every (seconds)", 60.0, 86_400.0)],
        "Window opened" | "Window closed" | "Application running?" | "Branch on application" => {
            vec![Text("Window class")]
        }
        "Delay" => vec![Number("Wait (seconds)", 1.0, 3600.0)],
        "Repeat" => vec![Number("Times", 1.0, 10.0)],
        "Wait for window" => vec![
            Text("Window class"),
            Number("Give up after (seconds)", 1.0, 3600.0),
        ],
        "Wait for entity" => vec![
            Entity("Entity", any_entity),
            Text("State"),
            Number("Give up after (seconds)", 1.0, 3600.0),
        ],
        "Notification" => vec![Text("Title"), Text("Message")],
        "Workspace" => vec![Number("Workspace", 1.0, 20.0)],
        "Volume" => vec![Number("Volume (%)", 0.0, 100.0)],
        "Launch application" => vec![App],
        "Entity state" | "Entity is?" => vec![Entity("Entity", any_entity), Text("State")],
        "Turn entity on/off" => vec![Entity("Device", switchable), OnOff],
        "Activate scene" => vec![Entity("Scene", scene)],
        "Set variable" | "Variable equals?" => vec![Text("Variable"), Text("Value")],
        _ => Vec::new(),
    }
}

fn snapshot(state: &mut CanvasState) {
    state.undo.push(state.workflow.clone());
    if state.undo.len() > 50 {
        state.undo.remove(0);
    }
    state.redo.clear();
    state.dry_results.clear();
    state.live_details.clear();
}

fn step_history(state: &mut CanvasState, back: bool) -> bool {
    let (from, to) = if back {
        (&mut state.undo, &mut state.redo)
    } else {
        (&mut state.redo, &mut state.undo)
    };
    let Some(other) = from.pop() else {
        return false;
    };
    to.push(std::mem::replace(&mut state.workflow, other));
    if state.undo.len() > 50 {
        state.undo.remove(0);
    }
    state.selection.clear();
    state.selected = None;
    state.connecting = None;
    true
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
                x: 36.0 + d as f64 * 264.0,
                y: 48.0 + *row as f64 * 120.0,
            },
        );
        *row += 1;
    }
}

fn hit(state: &CanvasState, x: f64, y: f64) -> Option<String> {
    let x = (x - state.pan.0) / state.zoom;
    let y = (y - state.pan.1) / state.zoom;
    for group in state.workflow.groups.iter().filter(|g| g.collapsed) {
        if let Some((gx, gy, _, _)) = group_bounds(&state.workflow, &group.members)
            && x >= gx
            && x <= gx + NODE_W
            && y >= gy
            && y <= gy + NODE_H
        {
            return group.members.first().cloned();
        }
    }
    state
        .workflow
        .nodes
        .iter()
        .rev()
        .find(|n| {
            collapsed_group(&state.workflow, &n.id).is_none()
                && state
                    .workflow
                    .positions
                    .get(&n.id)
                    .is_some_and(|p| x >= p.x && x <= p.x + NODE_W && y >= p.y && y <= p.y + NODE_H)
        })
        .map(|n| n.id.clone())
}

fn visible_point(workflow: &Workflow, id: &str) -> Option<Point> {
    if let Some(group) = collapsed_group(workflow, id) {
        let (x, y, _, _) = group_bounds(workflow, &group.members)?;
        Some(Point { x, y })
    } else {
        workflow.positions.get(id).copied()
    }
}

/// Make the whole graph fit the view.
fn fit(state: &mut CanvasState, width: f64, height: f64) {
    let points: Vec<_> = state.workflow.positions.values().collect();
    if points.is_empty() || width <= 0.0 || height <= 0.0 {
        return;
    }
    let x0 = points.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let y0 = points.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    let x1 = points
        .iter()
        .map(|p| p.x + NODE_W)
        .fold(f64::NEG_INFINITY, f64::max);
    let y1 = points
        .iter()
        .map(|p| p.y + NODE_H)
        .fold(f64::NEG_INFINITY, f64::max);
    let zoom = ((width - 80.0) / (x1 - x0))
        .min((height - 80.0) / (y1 - y0))
        .clamp(0.4, 1.5);
    state.zoom = zoom;
    state.pan = (
        (width - (x1 - x0) * zoom) / 2.0 - x0 * zoom,
        (height - (y1 - y0) * zoom) / 2.0 - y0 * zoom,
    );
}

struct Colors {
    bg: Rgb,
    card: Rgb,
    text: Rgb,
    dim: Rgb,
    accent: Rgb,
    danger: Rgb,
}

fn colors() -> Colors {
    let p = nyx_theme::palette();
    let bg = Rgb::hex(&p.bg);
    let text = Rgb::hex(&p.text);
    Colors {
        bg,
        card: bg.mix(text, if p.light { 0.04 } else { 0.07 }),
        text,
        dim: Rgb::hex(&p.dim_text),
        accent: Rgb::hex(&nyx_theme::app_accent("home")),
        danger: Rgb::hex(&p.danger),
    }
}

fn set(cr: &cairo::Context, c: Rgb, alpha: f64) {
    cr.set_source_rgba(c.0, c.1, c.2, alpha);
}

/// Draw `text` at most `width` wide, ending in "…" when it doesn't fit.
fn fitted_text(cr: &cairo::Context, text: &str, x: f64, y: f64, width: f64) {
    let fits = |s: &str| cr.text_extents(s).map_or(true, |e| e.x_advance() <= width);
    let mut shown = text.to_string();
    if !fits(&shown) {
        let mut chars: Vec<char> = text.chars().collect();
        while !chars.is_empty() {
            chars.pop();
            shown = chars.iter().collect::<String>() + "…";
            if fits(&shown) {
                break;
            }
        }
    }
    cr.move_to(x, y);
    let _ = cr.show_text(&shown);
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, PI + FRAC_PI_2);
    cr.close_path();
}

fn draw(state: &CanvasState, cr: &cairo::Context, width: i32, height: i32) {
    let c = colors();
    let font = nyx_theme::look()
        .font
        .ui
        .and_then(|list| list.split(',').next().map(|f| f.trim().to_string()))
        .unwrap_or_else(|| "Inter".into());
    set(cr, c.bg, 1.0);
    let _ = cr.paint();
    let _ = cr.save();
    cr.translate(state.pan.0, state.pan.1);
    cr.scale(state.zoom, state.zoom);
    let x0 = -state.pan.0 / state.zoom;
    let y0 = -state.pan.1 / state.zoom;
    let x1 = x0 + width as f64 / state.zoom;
    let y1 = y0 + height as f64 / state.zoom;
    set(cr, c.dim, 0.18);
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
    for group in &state.workflow.groups {
        let Some((gx, gy, gx1, gy1)) = group_bounds(&state.workflow, &group.members) else {
            continue;
        };
        let (w, h) = if group.collapsed {
            (NODE_W, NODE_H)
        } else {
            (gx1 - gx, gy1 - gy)
        };
        rounded(cr, gx, gy, w, h, 12.0);
        set(cr, c.accent, 0.08);
        let _ = cr.fill_preserve();
        set(cr, c.accent, 0.45);
        cr.set_line_width(1.0);
        let _ = cr.stroke();
        set(cr, c.text, 0.9);
        cr.select_font_face(&font, cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        cr.set_font_size(12.0);
        fitted_text(cr, &group.name, gx + 12.0, gy + 19.0, w - 24.0);
        if group.collapsed {
            cr.select_font_face(&font, cairo::FontSlant::Normal, cairo::FontWeight::Normal);
            set(cr, c.dim, 1.0);
            fitted_text(
                cr,
                &present::plural(group.members.len(), "step"),
                gx + 12.0,
                gy + 42.0,
                w - 24.0,
            );
        }
    }
    cr.select_font_face(&font, cairo::FontSlant::Normal, cairo::FontWeight::Normal);
    for edge in &state.workflow.edges {
        if collapsed_group(&state.workflow, &edge.from)
            .is_some_and(|g| g.members.contains(&edge.to))
        {
            continue;
        }
        let (Some(a), Some(b)) = (
            visible_point(&state.workflow, &edge.from),
            visible_point(&state.workflow, &edge.to),
        ) else {
            continue;
        };
        let (sx, sy, tx, ty) = (a.x + NODE_W, a.y + NODE_H / 2.0, b.x, b.y + NODE_H / 2.0);
        let tone = if edge.output.as_deref() == Some("error") {
            c.danger
        } else {
            c.accent
        };
        set(cr, tone, 0.75);
        cr.set_line_width(2.0);
        cr.move_to(sx, sy);
        cr.curve_to(sx + 70.0, sy, tx - 70.0, ty, tx, ty);
        let _ = cr.stroke();
        cr.arc(tx, ty, 3.5, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        if let Some(port) = &edge.output {
            cr.set_font_size(11.0);
            set(cr, tone, 1.0);
            let label = match port.as_str() {
                "true" => "Yes",
                "false" => "No",
                "error" => "On error",
                other => other,
            };
            cr.move_to(sx + 10.0, sy - 8.0);
            let _ = cr.show_text(label);
        }
    }
    for node in &state.workflow.nodes {
        if collapsed_group(&state.workflow, &node.id).is_some() {
            continue;
        }
        let Some(p) = state.workflow.positions.get(&node.id) else {
            continue;
        };
        let selected = state.selection.contains(&node.id);
        let connecting_from = state
            .connecting
            .as_ref()
            .is_some_and(|(from, _)| *from == node.id);
        rounded(cr, p.x, p.y, NODE_W, NODE_H, 10.0);
        set(
            cr,
            if selected {
                c.card.mix(c.accent, 0.14)
            } else {
                c.card
            },
            1.0,
        );
        let _ = cr.fill_preserve();
        let (outline, alpha, line) = match state.dry_results.get(&node.id).map(String::as_str) {
            Some("failed") => (c.danger, 1.0, 2.0),
            Some("running") => (c.accent, 1.0, 2.0),
            Some("skipped" | "canceled" | "interrupted") => (c.dim, 0.6, 1.5),
            Some("condition false" | "condition unknown") => (c.dim, 0.9, 1.5),
            Some(_) => (c.accent.mix(c.text, 0.25), 0.9, 1.5),
            None if selected || connecting_from => (c.accent, 1.0, 2.0),
            None => (c.text, 0.12, 1.0),
        };
        set(cr, outline, alpha);
        cr.set_line_width(line);
        let _ = cr.stroke();
        // A stripe at the left says what kind of step it is.
        let stripe = if node.kind.is_trigger() {
            c.accent
        } else if node.kind.is_condition() || matches!(node.kind, NodeKind::BranchAppRunning { .. })
        {
            c.accent.mix(c.text, 0.45)
        } else if node.kind.is_action() {
            c.accent.mix(c.bg, 0.35)
        } else {
            c.dim
        };
        rounded(cr, p.x + 6.0, p.y + 12.0, 3.0, NODE_H - 24.0, 1.5);
        set(cr, stripe, 1.0);
        let _ = cr.fill();
        cr.select_font_face(&font, cairo::FontSlant::Normal, cairo::FontWeight::Normal);
        cr.set_font_size(10.5);
        set(cr, c.dim, 1.0);
        let kind = match state.dry_results.get(&node.id) {
            Some(result) => format!("{} · {}", kind_name(&node.kind), result),
            None => kind_name(&node.kind).to_string(),
        };
        fitted_text(cr, &kind, p.x + 18.0, p.y + 24.0, NODE_W - 28.0);
        cr.select_font_face(&font, cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        cr.set_font_size(12.5);
        set(cr, c.text, 1.0);
        fitted_text(
            cr,
            &state.label(&node.kind),
            p.x + 18.0,
            p.y + 45.0,
            NODE_W - 28.0,
        );
    }
    let _ = cr.restore();
}

#[derive(Clone)]
struct Ui {
    state: Rc<RefCell<CanvasState>>,
    window: gtk::Window,
    area: gtk::DrawingArea,
    panel: gtk::Box,
    feedback: gtk::Label,
    banner: gtk::Revealer,
    banner_text: gtk::Label,
    owner: gtk::DropDown,
    palette: gtk::DropDown,
    live: gtk::Button,
    name: gtk::Entry,
}

impl Ui {
    fn say(&self, text: &str) {
        self.feedback.set_text(text);
    }
    fn changed(&self) {
        let connecting = self.state.borrow().connecting.clone();
        match connecting {
            Some((from, port)) => {
                let label = self.state.borrow().node_label(&from);
                self.banner_text.set_text(&match port.as_deref() {
                    Some("true") => {
                        format!("Click the step to run when the answer to “{label}” is yes.")
                    }
                    Some("false") => {
                        format!("Click the step to run when the answer to “{label}” is no.")
                    }
                    Some("error") => format!("Click the step to run if “{label}” fails."),
                    _ => format!("Click the step that follows “{label}”."),
                });
                self.banner.set_reveal_child(true);
            }
            None => self.banner.set_reveal_child(false),
        }
        self.sync_owner();
        self.area.queue_draw();
        inspector(self);
    }
    /// Keep the owner dropdown, palette and Live test in line with the workflow.
    fn sync_owner(&self) {
        let owner = self.state.borrow().workflow.owner;
        let index = match owner {
            Owner::Desktop => 0,
            Owner::HomeAssistant => 1,
            Owner::HybridDesktop => 2,
        };
        if self.owner.selected() != index {
            self.owner.set_selected(index);
        }
        self.live.set_sensitive(owner != Owner::HomeAssistant);
        let kinds = kinds_for(owner);
        let model = gtk::StringList::new(&kinds);
        let same = self
            .palette
            .model()
            .is_some_and(|m| m.n_items() as usize == kinds.len());
        if !same {
            self.palette.set_model(Some(&model));
            let default = kinds.iter().position(|k| *k == "Notification").unwrap_or(0);
            self.palette.set_selected(default as u32);
        }
    }
    fn palette_kind(&self) -> Option<&'static str> {
        let owner = self.state.borrow().workflow.owner;
        kinds_for(owner)
            .get(self.palette.selected() as usize)
            .copied()
    }
    /// Say what's wrong with `workflow` and select the step it's about.
    fn invalid(&self, workflow: &Workflow) -> bool {
        let Err(error) = workflow.validate() else {
            return false;
        };
        let explained = {
            let state = self.state.borrow();
            present::explain(&error, workflow, &|id| state.home.name_of(id))
        };
        if let (_, Some(id)) = nyx_home::automation::error_step(&error) {
            let mut state = self.state.borrow_mut();
            state.selection = HashSet::from([id.to_string()]);
            state.selected = Some(id.to_string());
        }
        self.changed();
        self.say(&explained);
        true
    }

    /// The workflow as it would be saved, with the name from the toolbar.
    fn current(&self) -> Workflow {
        let mut workflow = self.state.borrow().workflow.clone();
        workflow.name = self.name.text().trim().to_string();
        workflow
    }
}

fn clear(panel: &gtk::Box) {
    while let Some(child) = panel.first_child() {
        panel.remove(&child);
    }
}

fn field_label(text: &str) -> gtk::Label {
    widgets::label(text, "field-label")
}

fn inspector(ui: &Ui) {
    let panel = &ui.panel;
    clear(panel);
    panel.append(&widgets::label("Step", "card-title"));
    let Some(id) = ui.state.borrow().selected.clone() else {
        let hint = widgets::label(
            "Select a step to edit it or connect it to another. Drag Add step onto the canvas, or press it to add the chosen step.\n\nDrag steps to move them, drag empty space to pan, and scroll to zoom. Shift-click selects several.",
            "dim",
        );
        hint.set_wrap(true);
        panel.append(&hint);
        return;
    };
    let Some(node) = ui
        .state
        .borrow()
        .workflow
        .nodes
        .iter()
        .find(|n| n.id == id)
        .cloned()
    else {
        return;
    };
    let title = widgets::label(
        &ui.state.borrow().label(&node.kind),
        "settings-option-title",
    );
    title.set_wrap(true);
    panel.append(&title);
    if let Some(result) = ui.state.borrow().dry_results.get(&id) {
        panel.append(&widgets::tag(&present::sentence_case(result), ""));
    }
    if let Some(detail) = ui.state.borrow().live_details.get(&id) {
        let label = widgets::label(detail, "dim");
        label.set_wrap(true);
        panel.append(&label);
    }

    // ----- Type and parameters -----
    let owner = ui.state.borrow().workflow.owner;
    let kinds = kinds_for(owner);
    let current_kind = kind_name(&node.kind);
    let chooser = gtk::DropDown::from_strings(&kinds);
    chooser.update_property(&[gtk::accessible::Property::Label("Step type")]);
    if let Some(index) = kinds.iter().position(|k| *k == current_kind) {
        chooser.set_selected(index as u32);
    }
    let type_label = field_label("Type");
    chooser.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
        type_label.upcast_ref()
    ])]);
    panel.append(&type_label);
    panel.append(&chooser);
    let params = widgets::vbox(6);
    panel.append(&params);
    type Reader = Box<dyn Fn() -> String>;
    let readers: Rc<RefCell<Vec<Reader>>> = Rc::new(RefCell::new(Vec::new()));
    let fill = {
        let (params, readers, home) = (
            params.clone(),
            readers.clone(),
            ui.state.borrow().home.clone(),
        );
        move |name: &str, value: &str| {
            clear(&params);
            readers.borrow_mut().clear();
            let values: Vec<String> = if name == "Notification" {
                let (t, b) = value.split_once('|').unwrap_or((value, ""));
                vec![t.trim().into(), b.trim().into()]
            } else {
                value.split('|').map(|s| s.trim().to_string()).collect()
            };
            for (index, field) in fields(name).into_iter().enumerate() {
                let initial = values.get(index).cloned().unwrap_or_default();
                let reader: Reader =
                    match field {
                        Field::Text(label) => {
                            let l = field_label(label);
                            let entry = gtk::Entry::new();
                            entry.set_text(&initial);
                            entry.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
                                l.upcast_ref()
                            ])]);
                            params.append(&l);
                            params.append(&entry);
                            Box::new(move || entry.text().replace('|', "/"))
                        }
                        Field::Number(label, min, max) => {
                            let l = field_label(label);
                            let spin = gtk::SpinButton::with_range(min, max, 1.0);
                            spin.set_value(initial.parse().unwrap_or(min));
                            spin.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
                                l.upcast_ref()
                            ])]);
                            params.append(&l);
                            params.append(&spin);
                            Box::new(move || (spin.value() as u64).to_string())
                        }
                        Field::Entity(label, keep) => {
                            let l = field_label(label);
                            let picker = Picker::entities(&home, keep);
                            picker.set(&initial);
                            picker.widget.update_relation(&[
                                gtk::accessible::Relation::LabelledBy(&[l.upcast_ref()]),
                            ]);
                            params.append(&l);
                            params.append(&picker.widget);
                            if home.entities.is_empty() {
                                params.append(&widgets::label(
                                    "Connect Home Assistant to choose one.",
                                    "warning-text",
                                ));
                            }
                            Box::new(move || picker.value())
                        }
                        Field::App => {
                            let l = field_label("App");
                            let entry = gtk::Entry::new();
                            entry.set_text(&initial);
                            entry.set_placeholder_text(Some("Desktop file, e.g. firefox.desktop"));
                            entry.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
                                l.upcast_ref()
                            ])]);
                            params.append(&l);
                            params.append(&entry);
                            Box::new(move || entry.text().to_string())
                        }
                        Field::OnOff => {
                            let switch_row = widgets::hbox(8);
                            let l = widgets::label("Turn on", "");
                            l.set_hexpand(true);
                            let switch = gtk::Switch::new();
                            switch.set_active(initial != "off");
                            switch.update_property(&[gtk::accessible::Property::Label(
                                "Turn on (off turns it off)",
                            )]);
                            switch_row.append(&l);
                            switch_row.append(&switch);
                            params.append(&switch_row);
                            Box::new(move || {
                                if switch.is_active() {
                                    "on".into()
                                } else {
                                    "off".into()
                                }
                            })
                        }
                    };
                readers.borrow_mut().push(reader);
            }
        }
    };
    let fill = Rc::new(fill);
    fill(current_kind, &kind_value(&node.kind));
    {
        let (fill, kinds, original) = (fill.clone(), kinds.clone(), node.kind.clone());
        chooser.connect_selected_notify(move |dropdown| {
            if let Some(name) = kinds.get(dropdown.selected() as usize) {
                let value = if *name == current_kind {
                    kind_value(&original)
                } else {
                    default_value(name).to_string()
                };
                fill(name, &value);
            }
        });
    }
    let apply = gtk::Button::with_label("Apply changes");
    apply.add_css_class("suggested-action");
    panel.append(&apply);
    {
        let (ui2, id, kinds) = (ui.clone(), id.clone(), kinds.clone());
        apply.connect_clicked(move |_| {
            let Some(name) = kinds.get(chooser.selected() as usize).copied() else {
                ui2.say("Choose a step type.");
                return;
            };
            let value = readers
                .borrow()
                .iter()
                .map(|r| r())
                .collect::<Vec<_>>()
                .join(" | ");
            match make_kind(name, &value) {
                Ok(kind) => {
                    {
                        let mut s = ui2.state.borrow_mut();
                        snapshot(&mut s);
                        let branch = matches!(kind, NodeKind::BranchAppRunning { .. });
                        let action = kind.is_action();
                        if !action
                            || (!kind.supports_retry()
                                && s.workflow.policies.get(&id).is_some_and(|p| p.retries > 0))
                        {
                            s.workflow.policies.remove(&id);
                        }
                        for edge in s.workflow.edges.iter_mut().filter(|e| e.from == id) {
                            match edge.output.as_deref() {
                                Some("true" | "false") if !branch => edge.output = None,
                                Some("error") if !action => edge.output = None,
                                _ => {}
                            }
                        }
                        if let Some(node) = s.workflow.nodes.iter_mut().find(|n| n.id == id) {
                            node.kind = kind;
                        }
                    }
                    ui2.say("Step updated. Save to keep it.");
                    ui2.changed();
                }
                Err(e) => ui2.say(&e),
            }
        });
    }

    // ----- Action policy -----
    if owner != Owner::HomeAssistant && node.kind.is_action() {
        let current = ui
            .state
            .borrow()
            .workflow
            .policies
            .get(&id)
            .cloned()
            .unwrap_or(ActionPolicy {
                retries: 0,
                timeout_seconds: 10,
            });
        panel.append(&widgets::label("If it's slow or fails", "card-title"));
        let timeout_label = field_label("Give up after (seconds)");
        let timeout = gtk::SpinButton::with_range(1.0, 60.0, 1.0);
        timeout.set_value(current.timeout_seconds as f64);
        timeout.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
            timeout_label.upcast_ref()
        ])]);
        let retries_label = field_label("Try again (times)");
        let retries = gtk::SpinButton::with_range(0.0, 3.0, 1.0);
        retries.set_value(current.retries as f64);
        retries.set_sensitive(node.kind.supports_retry());
        retries.update_relation(&[gtk::accessible::Relation::LabelledBy(&[
            retries_label.upcast_ref()
        ])]);
        if !node.kind.supports_retry() {
            retries.set_tooltip_text(Some("Only steps that are safe to repeat can try again"));
        }
        panel.append(&timeout_label);
        panel.append(&timeout);
        panel.append(&retries_label);
        panel.append(&retries);
        let apply_policy = gtk::Button::with_label("Apply");
        panel.append(&apply_policy);
        let (ui2, id2) = (ui.clone(), id.clone());
        apply_policy.connect_clicked(move |_| {
            let mut state = ui2.state.borrow_mut();
            snapshot(&mut state);
            state.workflow.policies.insert(
                id2.clone(),
                ActionPolicy {
                    timeout_seconds: timeout.value() as u64,
                    retries: retries.value() as u8,
                },
            );
            drop(state);
            ui2.say("Timeout and retries updated.");
        });
    }

    // ----- Connections -----
    panel.append(&widgets::label("Connections", "card-title"));
    let outgoing: Vec<Edge> = ui
        .state
        .borrow()
        .workflow
        .edges
        .iter()
        .filter(|e| e.from == id)
        .cloned()
        .collect();
    if outgoing.is_empty() {
        panel.append(&widgets::label("Nothing follows this step yet.", "dim"));
    }
    for edge in outgoing {
        let row = widgets::hbox(6);
        let port = match edge.output.as_deref() {
            Some("true") => "If yes",
            Some("false") => "If no",
            Some("error") => "On error",
            _ => "Then",
        };
        let text = widgets::label(
            &format!("{port}: {}", ui.state.borrow().node_label(&edge.to)),
            "",
        );
        text.set_wrap(true);
        text.set_hexpand(true);
        row.append(&text);
        let remove = widgets::icon_button("list-remove-symbolic", "Remove this connection");
        let ui2 = ui.clone();
        remove.connect_clicked(move |_| {
            {
                let mut state = ui2.state.borrow_mut();
                snapshot(&mut state);
                state
                    .workflow
                    .edges
                    .retain(|e| !(e.from == edge.from && e.to == edge.to));
            }
            ui2.changed();
        });
        row.append(&remove);
        panel.append(&row);
    }
    let ports: Vec<(&str, Option<&str>)> = if matches!(node.kind, NodeKind::BranchAppRunning { .. })
    {
        vec![
            ("Connect “yes”…", Some("true")),
            ("Connect “no”…", Some("false")),
        ]
    } else if node.kind.is_action() && owner != Owner::HomeAssistant {
        vec![
            ("Connect next step…", None),
            ("Connect error handler…", Some("error")),
        ]
    } else {
        vec![("Connect next step…", None)]
    };
    for (label, port) in ports {
        let button = gtk::Button::with_label(label);
        let (ui2, id2) = (ui.clone(), id.clone());
        let port = port.map(str::to_string);
        button.connect_clicked(move |_| {
            ui2.state.borrow_mut().connecting = Some((id2.clone(), port.clone()));
            ui2.changed();
            ui2.area.grab_focus();
        });
        panel.append(&button);
    }

    // ----- Groups -----
    if ui.state.borrow().selection.len() >= 2 {
        let group_button = gtk::Button::with_label("Group selected steps");
        let ui2 = ui.clone();
        group_button.connect_clicked(move |_| {
            {
                let mut state = ui2.state.borrow_mut();
                if state
                    .selection
                    .iter()
                    .any(|id| state.workflow.groups.iter().any(|g| g.members.contains(id)))
                {
                    drop(state);
                    ui2.say("Ungroup those steps first.");
                    return;
                }
                let members: Vec<_> = state.selection.iter().cloned().collect();
                snapshot(&mut state);
                let number = state.workflow.groups.len() + 1;
                state.workflow.groups.push(Group {
                    id: format!("group-{}", uuid::Uuid::new_v4().simple()),
                    name: format!("Group {number}"),
                    members,
                    collapsed: false,
                });
            }
            ui2.changed();
        });
        panel.append(&group_button);
    }
    let group = ui
        .state
        .borrow()
        .workflow
        .groups
        .iter()
        .find(|g| g.members.contains(&id))
        .cloned();
    if let Some(group) = group {
        panel.append(&widgets::label("Group", "card-title"));
        let group_name = gtk::Entry::new();
        group_name.set_text(&group.name);
        group_name.update_property(&[gtk::accessible::Property::Label("Group name")]);
        panel.append(&group_name);
        let buttons = widgets::hbox(6);
        let rename = gtk::Button::with_label("Rename");
        let collapse = gtk::Button::with_label(if group.collapsed {
            "Expand"
        } else {
            "Collapse"
        });
        let ungroup = gtk::Button::with_label("Ungroup");
        buttons.append(&rename);
        buttons.append(&collapse);
        buttons.append(&ungroup);
        panel.append(&buttons);
        let (ui2, gid) = (ui.clone(), group.id.clone());
        rename.connect_clicked(move |_| {
            let name = group_name.text().trim().to_string();
            if name.is_empty() || name.len() > 80 {
                ui2.say("Enter a group name of up to 80 characters.");
                return;
            }
            {
                let mut state = ui2.state.borrow_mut();
                snapshot(&mut state);
                if let Some(g) = state.workflow.groups.iter_mut().find(|g| g.id == gid) {
                    g.name = name;
                }
            }
            ui2.changed();
        });
        let (ui2, gid) = (ui.clone(), group.id.clone());
        collapse.connect_clicked(move |_| {
            {
                let mut state = ui2.state.borrow_mut();
                snapshot(&mut state);
                if let Some(g) = state.workflow.groups.iter_mut().find(|g| g.id == gid) {
                    g.collapsed = !g.collapsed;
                }
            }
            ui2.changed();
        });
        let (ui2, gid) = (ui.clone(), group.id.clone());
        ungroup.connect_clicked(move |_| {
            {
                let mut state = ui2.state.borrow_mut();
                snapshot(&mut state);
                state.workflow.groups.retain(|g| g.id != gid);
            }
            ui2.changed();
        });
    }

    let several = ui.state.borrow().selection.len() > 1;
    let remove = gtk::Button::with_label(if several {
        "Remove selected steps"
    } else {
        "Remove step"
    });
    remove.add_css_class("destructive-action");
    remove.set_margin_top(8);
    let ui2 = ui.clone();
    remove.connect_clicked(move |_| {
        delete_selection(&mut ui2.state.borrow_mut());
        ui2.say("Removed. Reconnect any steps that were left on their own.");
        ui2.changed();
    });
    panel.append(&remove);
}

fn icon(name: &str, tip: &str) -> gtk::Button {
    let button = widgets::bar_button(name, tip);
    button.update_property(&[gtk::accessible::Property::Label(
        tip.split(" (").next().unwrap_or(tip),
    )]);
    button
}

fn separator() -> gtk::Separator {
    gtk::Separator::new(gtk::Orientation::Vertical)
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
    let saved = Rc::new(RefCell::new(
        serde_json::to_string(&workflow).unwrap_or_default(),
    ));
    let state = Rc::new(RefCell::new(CanvasState {
        workflow,
        selected: None,
        selection: HashSet::new(),
        clipboard: None,
        connecting: None,
        pan: (0.0, 0.0),
        zoom: 1.0,
        pointer: (0.0, 0.0),
        undo: vec![],
        redo: vec![],
        dry_results: HashMap::new(),
        live_details: HashMap::new(),
        home: home.clone(),
    }));
    let window = gtk::Window::builder()
        .title(format!("{} — Canvas", state.borrow().workflow.name))
        .transient_for(parent)
        .default_width(1240)
        .default_height(780)
        .build();
    // Part of the app: it shares Home's window class and keeps the app running.
    window.set_application(parent.application().as_ref());
    window.add_css_class("home-window");
    window.set_titlebar(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));

    let header = nyx_ui::chrome::app_header(&window, crate::icons::AUTOMATIONS, "Canvas");
    let name = gtk::Entry::new();
    name.set_text(&state.borrow().workflow.name);
    name.set_width_chars(26);
    name.set_placeholder_text(Some("Automation name"));
    name.update_property(&[gtk::accessible::Property::Label("Automation name")]);
    header.start.append(&name);
    let owner =
        gtk::DropDown::from_strings(&["Runs on this computer", "Runs in Home Assistant", "Hybrid"]);
    owner.update_property(&[gtk::accessible::Property::Label("Where it runs")]);
    owner.set_sensitive(state.borrow().workflow.owner != Owner::HomeAssistant);
    owner.set_tooltip_text(Some("Where the automation runs"));
    header.start.append(&owner);
    let close = gtk::Button::with_label("Cancel");
    close.set_tooltip_text(Some("Close the canvas (Esc)"));
    {
        let window = window.clone();
        close.connect_clicked(move |_| window.close());
    }
    header.end.set_spacing(8);
    header.end.append(&close);
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    save.set_tooltip_text(Some("Save (Ctrl+S)"));
    header.end.append(&save);

    let toolbar = widgets::hbox(2);
    toolbar.add_css_class("canvas-toolbar");
    let palette = gtk::DropDown::from_strings(&[]);
    palette.update_property(&[gtk::accessible::Property::Label("Step to add")]);
    palette.set_tooltip_text(Some("The step Add step places"));
    let add = widgets::labeled_button("list-add-symbolic", "Add step");
    add.set_tooltip_text(Some("Add the chosen step, or drag this onto the canvas"));
    toolbar.append(&palette);
    toolbar.append(&add);
    toolbar.append(&separator());
    let undo = icon("edit-undo-symbolic", "Undo (Ctrl+Z)");
    let redo = icon("edit-redo-symbolic", "Redo (Ctrl+Shift+Z)");
    let copy = icon("edit-copy-symbolic", "Copy (Ctrl+C)");
    let paste = icon("edit-paste-symbolic", "Paste (Ctrl+V)");
    for b in [&undo, &redo, &copy, &paste] {
        toolbar.append(b);
    }
    toolbar.append(&separator());
    let auto = icon("view-grid-symbolic", "Tidy up the layout");
    let fit_button = icon("zoom-fit-best-symbolic", "Fit to view (Ctrl+0)");
    toolbar.append(&auto);
    toolbar.append(&fit_button);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    toolbar.append(&spacer);
    let dry = widgets::labeled_button("system-run-symbolic", "Dry run");
    dry.set_tooltip_text(Some("Walk through the steps without doing anything"));
    let live = widgets::labeled_button("media-playback-start-symbolic", "Live test");
    live.set_tooltip_text(Some("Run it once for real, without saving"));
    let cancel = icon("media-playback-stop-symbolic", "Stop the test run");
    cancel.set_sensitive(false);
    toolbar.append(&dry);
    toolbar.append(&live);
    toolbar.append(&cancel);

    let banner_text = widgets::label("", "");
    banner_text.set_hexpand(true);
    banner_text.set_wrap(true);
    let banner_box = widgets::hbox(10);
    banner_box.add_css_class("banner");
    banner_box.add_css_class("canvas-banner");
    banner_box.append(&gtk::Image::from_icon_name("dialog-information-symbolic"));
    banner_box.append(&banner_text);
    let stop_connecting = gtk::Button::with_label("Cancel (Esc)");
    banner_box.append(&stop_connecting);
    let banner = gtk::Revealer::builder().child(&banner_box).build();

    let area = gtk::DrawingArea::new();
    area.add_css_class("canvas-area");
    area.set_hexpand(true);
    area.set_vexpand(true);
    area.set_focusable(true);
    area.set_accessible_role(gtk::AccessibleRole::Img);
    let panel = widgets::vbox(8);
    let inspector_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .child(&panel)
        .build();
    inspector_scroll.add_css_class("canvas-inspector");
    inspector_scroll.set_width_request(300);
    let body = widgets::hbox(0);
    body.set_vexpand(true);
    let column = widgets::vbox(0);
    column.set_hexpand(true);
    let feedback = widgets::label(
        "Drag steps to move them. Scroll to zoom. Drag empty space to pan.",
        "canvas-feedback",
    );
    feedback.set_ellipsize(gtk::pango::EllipsizeMode::End);
    feedback.set_accessible_role(gtk::AccessibleRole::Status);
    column.append(&banner);
    column.append(&area);
    column.append(&feedback);
    body.append(&column);
    body.append(&inspector_scroll);

    let frame = widgets::vbox(0);
    frame.add_css_class("window-frame");
    frame.append(&header.root);
    frame.append(&toolbar);
    frame.append(&body);
    window.set_child(Some(&frame));

    let ui = Ui {
        state: state.clone(),
        window: window.clone(),
        area: area.clone(),
        panel,
        feedback,
        banner,
        banner_text,
        owner: owner.clone(),
        palette: palette.clone(),
        live: live.clone(),
        name: name.clone(),
    };
    ui.changed();
    {
        let s = state.clone();
        area.set_draw_func(move |area, cr, w, h| {
            draw(&s.borrow(), cr, w, h);
            let summary = {
                let s = s.borrow();
                format!(
                    "Automation graph with {}: {}",
                    present::plural(s.workflow.nodes.len(), "step"),
                    s.workflow
                        .nodes
                        .iter()
                        .map(|n| s.label(&n.kind))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            area.update_property(&[gtk::accessible::Property::Description(&summary)]);
        });
    }
    {
        let ui2 = ui.clone();
        owner.connect_selected_notify(move |drop| {
            let owner = match drop.selected() {
                0 => Owner::Desktop,
                1 => Owner::HomeAssistant,
                _ => Owner::HybridDesktop,
            };
            let changed = {
                let mut state = ui2.state.borrow_mut();
                let changed = state.workflow.owner != owner;
                if changed {
                    snapshot(&mut state);
                    state.workflow.owner = owner;
                }
                changed
            };
            if changed {
                ui2.changed();
            }
        });
    }
    {
        let ui2 = ui.clone();
        stop_connecting.connect_clicked(move |_| {
            ui2.state.borrow_mut().connecting = None;
            ui2.changed();
        });
    }

    // ----- Pointer -----
    let click = gtk::GestureClick::new();
    {
        let ui2 = ui.clone();
        click.connect_pressed(move |gesture, _, x, y| {
            ui2.area.grab_focus();
            let target = hit(&ui2.state.borrow(), x, y);
            let source = ui2.state.borrow_mut().connecting.take();
            if let Some((from, port)) = source {
                match target.clone() {
                    Some(to) if to != from => {
                        let replaced = {
                            let mut state = ui2.state.borrow_mut();
                            if !state.workflow.nodes.iter().any(|n| n.id == from) {
                                None
                            } else {
                                snapshot(&mut state);
                                let replaced = state
                                    .workflow
                                    .edges
                                    .iter()
                                    .find(|e| e.to == to)
                                    .map(|e| e.from.clone());
                                state.workflow.edges.retain(|e| e.to != to);
                                state
                                    .workflow
                                    .edges
                                    .retain(|e| !(e.from == from && port.is_some() && e.output == port));
                                state.workflow.edges.push(Edge { from: from.clone(), to: to.clone(), output: port });
                                Some(replaced)
                            }
                        };
                        match replaced {
                            Some(Some(previous)) if previous != from => {
                                let label = ui2.state.borrow().node_label(&previous);
                                ui2.say(&format!(
                                    "Connected. A step can follow only one other, so it no longer follows “{label}”."
                                ));
                            }
                            Some(_) => ui2.say("Connected."),
                            None => ui2.say("That step was removed; nothing was connected."),
                        }
                    }
                    _ => ui2.say("Nothing was connected."),
                }
            }
            let shift = gesture.current_event_state().contains(gdk::ModifierType::SHIFT_MASK);
            {
                let mut state = ui2.state.borrow_mut();
                if !shift {
                    state.selection.clear();
                }
                if let Some(id) = &target {
                    if let Some(group) = collapsed_group(&state.workflow, id) {
                        let members = group.members.clone();
                        state.selection.extend(members);
                    } else if shift && state.selection.contains(id) {
                        state.selection.remove(id);
                    } else {
                        state.selection.insert(id.clone());
                    }
                }
                state.selected = target.filter(|id| state.selection.contains(id));
            }
            ui2.changed();
        });
    }
    area.add_controller(click);

    let drag = gtk::GestureDrag::new();
    type Origin = (
        Option<String>,
        HashMap<String, Point>,
        (f64, f64),
        Option<Workflow>,
    );
    let origin: Rc<RefCell<Origin>> =
        Rc::new(RefCell::new((None, HashMap::new(), (0.0, 0.0), None)));
    {
        let (s, o) = (state.clone(), origin.clone());
        drag.connect_drag_begin(move |_, x, y| {
            let state = s.borrow();
            let id = hit(&state, x, y);
            let ids: HashSet<String> = match &id {
                Some(id) if state.selection.contains(id) => state.selection.clone(),
                Some(id) => HashSet::from([id.clone()]),
                None => HashSet::new(),
            };
            let points = state
                .workflow
                .positions
                .iter()
                .filter(|(id, _)| ids.contains(*id))
                .map(|(id, p)| (id.clone(), *p))
                .collect();
            *o.borrow_mut() = (id, points, state.pan, Some(state.workflow.clone()));
        });
    }
    {
        let (s, o, a) = (state.clone(), origin.clone(), area.clone());
        drag.connect_drag_update(move |_, dx, dy| {
            let origin = o.borrow();
            let mut state = s.borrow_mut();
            if origin.0.is_some() {
                let zoom = state.zoom;
                for (id, start) in &origin.1 {
                    if let Some(p) = state.workflow.positions.get_mut(id) {
                        p.x = snap(start.x + dx / zoom);
                        p.y = snap(start.y + dy / zoom);
                    }
                }
            } else {
                state.pan = (origin.2.0 + dx, origin.2.1 + dy);
            }
            a.queue_draw();
        });
    }
    {
        let (s, o) = (state.clone(), origin.clone());
        drag.connect_drag_end(move |_, dx, dy| {
            if (dx.abs() > 2.0 || dy.abs() > 2.0)
                && o.borrow().0.is_some()
                && let Some(before) = o.borrow_mut().3.take()
            {
                let mut state = s.borrow_mut();
                state.undo.push(before);
                if state.undo.len() > 50 {
                    state.undo.remove(0);
                }
                state.redo.clear();
            }
        });
    }
    area.add_controller(drag);

    let motion = gtk::EventControllerMotion::new();
    {
        let s = state.clone();
        motion.connect_motion(move |_, x, y| s.borrow_mut().pointer = (x, y));
    }
    area.add_controller(motion);
    let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    {
        let (s, a) = (state.clone(), area.clone());
        scroll.connect_scroll(move |_, _, dy| {
            let mut state = s.borrow_mut();
            let old = state.zoom;
            let zoom = (old * if dy > 0.0 { 0.9 } else { 1.1 }).clamp(0.4, 2.5);
            let (px, py) = state.pointer;
            state.pan = (
                px - (px - state.pan.0) * zoom / old,
                py - (py - state.pan.1) * zoom / old,
            );
            state.zoom = zoom;
            a.queue_draw();
            glib::Propagation::Stop
        });
    }
    area.add_controller(scroll);

    // ----- Adding steps -----
    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gdk::DragAction::COPY);
    {
        let ui2 = ui.clone();
        drag_source.connect_prepare(move |_, _, _| {
            let name = ui2.palette_kind()?;
            Some(gdk::ContentProvider::for_value(
                &format!("nyx-home-node:{name}").to_value(),
            ))
        });
    }
    add.add_controller(drag_source);
    let drop_target = gtk::DropTarget::new(String::static_type(), gdk::DragAction::COPY);
    {
        let ui2 = ui.clone();
        drop_target.connect_drop(move |_, value, x, y| {
            let Ok(text) = value.get::<String>() else {
                return false;
            };
            let Some(name) = text.strip_prefix("nyx-home-node:") else {
                return false;
            };
            let (px, py, zoom) = {
                let state = ui2.state.borrow();
                (state.pan.0, state.pan.1, state.zoom)
            };
            let added = add_node(
                &mut ui2.state.borrow_mut(),
                name,
                Point {
                    x: (x - px) / zoom,
                    y: (y - py) / zoom,
                },
            );
            ui2.say(if added {
                "Added. Connect it to the rest."
            } else {
                "Couldn't add that step: a graph holds up to 64."
            });
            ui2.changed();
            added
        });
    }
    area.add_controller(drop_target);
    {
        let ui2 = ui.clone();
        add.connect_clicked(move |_| {
            let Some(name) = ui2.palette_kind() else {
                return;
            };
            let point = {
                let state = ui2.state.borrow();
                let (w, h) = (ui2.area.width() as f64, ui2.area.height() as f64);
                let count = state.workflow.nodes.len() as f64;
                Point {
                    x: (w / 2.0 - state.pan.0) / state.zoom - NODE_W / 2.0 + (count % 3.0) * 24.0,
                    y: (h / 2.0 - state.pan.1) / state.zoom - NODE_H / 2.0 + (count % 3.0) * 24.0,
                }
            };
            let added = add_node(&mut ui2.state.borrow_mut(), name, point);
            ui2.say(if added {
                "Added. Connect it to the rest."
            } else {
                "Couldn't add that step: a graph holds up to 64."
            });
            ui2.changed();
        });
    }

    // ----- Toolbar actions -----
    {
        let ui2 = ui.clone();
        auto.connect_clicked(move |_| {
            {
                let mut state = ui2.state.borrow_mut();
                snapshot(&mut state);
                layout(&mut state.workflow);
            }
            ui2.changed();
        });
    }
    let fit_view = {
        let ui2 = ui.clone();
        move || {
            let (w, h) = (ui2.area.width() as f64, ui2.area.height() as f64);
            fit(&mut ui2.state.borrow_mut(), w, h);
            ui2.area.queue_draw();
        }
    };
    let fit_view = Rc::new(fit_view);
    {
        let fit_view = fit_view.clone();
        fit_button.connect_clicked(move |_| fit_view());
    }
    {
        let fit_view = fit_view.clone();
        area.connect_resize(move |_, _, _| {
            let fit_view = fit_view.clone();
            glib::idle_add_local_once(move || fit_view());
        });
    }
    let history = |ui: &Ui, back: bool| {
        if step_history(&mut ui.state.borrow_mut(), back) {
            ui.changed();
        }
    };
    {
        let ui2 = ui.clone();
        undo.connect_clicked(move |_| history(&ui2, true));
    }
    {
        let ui2 = ui.clone();
        redo.connect_clicked(move |_| history(&ui2, false));
    }
    {
        let ui2 = ui.clone();
        copy.connect_clicked(move |_| {
            copy_selection(&mut ui2.state.borrow_mut());
            ui2.say("Copied.");
        });
    }
    {
        let ui2 = ui.clone();
        paste.connect_clicked(move |_| {
            let pasted = paste_selection(&mut ui2.state.borrow_mut());
            ui2.say(if pasted {
                "Pasted. Connect the steps to the rest."
            } else {
                "Nothing to paste, or the graph is full."
            });
            ui2.changed();
        });
    }

    // ----- Keys -----
    // Before GTK's own focus keys, so Tab walks steps while the canvas has focus.
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    {
        let (ui2, save) = (ui.clone(), save.clone());
        let fit_view = fit_view.clone();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
            if ctrl && key == gdk::Key::s {
                save.emit_clicked();
                return glib::Propagation::Stop;
            }
            let typing = gtk::prelude::GtkWindowExt::focus(&ui2.window).is_some_and(|w| {
                w.is::<gtk::Text>()
                    || w.ancestor(gtk::Entry::static_type()).is_some()
                    || w.ancestor(gtk::SpinButton::static_type()).is_some()
                    || w.ancestor(gtk::DropDown::static_type()).is_some()
            });
            if typing {
                return glib::Propagation::Proceed;
            }
            let on_canvas = ui2.area.has_focus();
            let ids: Vec<String> = ui2
                .state
                .borrow()
                .workflow
                .nodes
                .iter()
                .map(|n| n.id.clone())
                .collect();
            let changed = match key {
                gdk::Key::a if ctrl => {
                    let mut state = ui2.state.borrow_mut();
                    state.selection = ids.iter().cloned().collect();
                    state.selected = ids.first().cloned();
                    true
                }
                gdk::Key::c if ctrl => {
                    copy_selection(&mut ui2.state.borrow_mut());
                    ui2.say("Copied.");
                    false
                }
                gdk::Key::v if ctrl => paste_selection(&mut ui2.state.borrow_mut()),
                gdk::Key::z | gdk::Key::Z if ctrl => {
                    step_history(&mut ui2.state.borrow_mut(), !shift)
                }
                gdk::Key::y if ctrl => step_history(&mut ui2.state.borrow_mut(), false),
                gdk::Key::_0 if ctrl => {
                    fit_view();
                    false
                }
                gdk::Key::Delete | gdk::Key::BackSpace if on_canvas => {
                    delete_selection(&mut ui2.state.borrow_mut());
                    true
                }
                gdk::Key::Escape => {
                    let mut state = ui2.state.borrow_mut();
                    if state.connecting.is_some() {
                        state.connecting = None;
                    } else if !state.selection.is_empty() {
                        state.selection.clear();
                        state.selected = None;
                    } else if on_canvas {
                        drop(state);
                        ui2.window.close();
                        return glib::Propagation::Stop;
                    } else {
                        return glib::Propagation::Proceed;
                    }
                    true
                }
                // Tab and Shift+Tab walk the steps while the canvas has focus.
                gdk::Key::Tab | gdk::Key::ISO_Left_Tab if on_canvas && !ids.is_empty() => {
                    let mut state = ui2.state.borrow_mut();
                    let at = state
                        .selected
                        .as_ref()
                        .and_then(|s| ids.iter().position(|i| i == s));
                    let back = key == gdk::Key::ISO_Left_Tab || shift;
                    let next = match at {
                        None => 0,
                        Some(i) if back => (i + ids.len() - 1) % ids.len(),
                        Some(i) => (i + 1) % ids.len(),
                    };
                    state.selection = HashSet::from([ids[next].clone()]);
                    state.selected = Some(ids[next].clone());
                    true
                }
                gdk::Key::Left | gdk::Key::Right | gdk::Key::Up | gdk::Key::Down if on_canvas => {
                    let (dx, dy) = match key {
                        gdk::Key::Left => (-GRID, 0.0),
                        gdk::Key::Right => (GRID, 0.0),
                        gdk::Key::Up => (0.0, -GRID),
                        _ => (0.0, GRID),
                    };
                    let mut state = ui2.state.borrow_mut();
                    if state.selection.is_empty() {
                        state.pan = (state.pan.0 - dx * 4.0, state.pan.1 - dy * 4.0);
                    } else {
                        snapshot(&mut state);
                        let selected = state.selection.clone();
                        for id in selected {
                            if let Some(p) = state.workflow.positions.get_mut(&id) {
                                p.x += dx;
                                p.y += dy;
                            }
                        }
                    }
                    true
                }
                _ => return glib::Propagation::Proceed,
            };
            if changed {
                ui2.changed();
            }
            glib::Propagation::Stop
        });
    }
    window.add_controller(keys);

    // ----- Dry run and live test -----
    {
        let ui2 = ui.clone();
        dry.connect_clicked(move |_| {
            let context = DryRunContext {
                entity_states: ui2
                    .state
                    .borrow()
                    .home
                    .entities
                    .iter()
                    .map(|(id, _, state)| (id.clone(), state.clone()))
                    .collect(),
                running_apps: None,
            };
            let result = ui2.state.borrow().workflow.dry_run(&context);
            match result {
                Ok(report) => {
                    ui2.state.borrow_mut().dry_results = report
                        .steps
                        .iter()
                        .map(|step| (step.node_id.clone(), step.result.clone()))
                        .collect();
                    let ran = report
                        .steps
                        .iter()
                        .filter(|s| !s.result.contains("skip"))
                        .count();
                    let mut text = format!(
                        "Dry run: {} of {} would run. Results are on each step.",
                        ran,
                        present::plural(report.steps.len(), "step")
                    );
                    if !report.warnings.is_empty() {
                        text.push(' ');
                        text.push_str(&report.warnings.join(" "));
                    }
                    ui2.say(&text);
                    ui2.changed();
                }
                Err(e) => ui2.say(&e),
            }
        });
    }
    let run_id = Rc::new(RefCell::new(None::<String>));
    let open = Rc::new(Cell::new(true));
    {
        let (ui2, cancel, run_id, open) =
            (ui.clone(), cancel.clone(), run_id.clone(), open.clone());
        live.connect_clicked(move |live| {
            let workflow = ui2.current();
            if workflow.owner == Owner::HomeAssistant {
                ui2.say("Home Assistant automations are tested from the automation list.");
                return;
            }
            if ui2.invalid(&workflow) {
                return;
            }
            ui2.say("Starting a live test…");
            live.set_sensitive(false);
            let (ui3, cancel, run_id, open, live) = (
                ui2.clone(),
                cancel.clone(),
                run_id.clone(),
                open.clone(),
                live.clone(),
            );
            rt::spawn(
                async move { service::run_draft(&workflow).await },
                move |result| {
                    let id = match result {
                        Ok(id) => id,
                        Err(e) => {
                            ui3.say(&e);
                            live.set_sensitive(true);
                            return;
                        }
                    };
                    *run_id.borrow_mut() = Some(id.clone());
                    cancel.set_sensitive(true);
                    glib::spawn_future_local(async move {
                        while open.get() {
                            let (tx, rx) = async_channel::bounded(1);
                            let query = id.clone();
                            rt::runtime().spawn(async move {
                                let _ = tx.send(service::progress(&query).await).await;
                            });
                            let progress = match rx.recv().await {
                                Ok(Ok(progress)) => progress,
                                Ok(Err(e)) => {
                                    ui3.say(&e);
                                    break;
                                }
                                Err(_) => break,
                            };
                            {
                                let mut state = ui3.state.borrow_mut();
                                state.dry_results = progress
                                    .nodes
                                    .iter()
                                    .map(|n| (n.node_id.clone(), n.status.clone()))
                                    .collect();
                                state.live_details = progress
                                    .nodes
                                    .iter()
                                    .map(|n| (n.node_id.clone(), n.detail.clone()))
                                    .collect();
                            }
                            ui3.area.queue_draw();
                            let (status, _, _) = present::status(&progress.status);
                            ui3.say(&format!("Live test: {}", status.to_lowercase()));
                            if progress.status != "running" {
                                break;
                            }
                            glib::timeout_future(Duration::from_millis(500)).await;
                        }
                        *run_id.borrow_mut() = None;
                        cancel.set_sensitive(false);
                        live.set_sensitive(true);
                        if open.get() {
                            ui3.changed();
                        }
                    });
                },
            );
        });
    }
    {
        let (ui2, run_id) = (ui.clone(), run_id.clone());
        cancel.connect_clicked(move |_| {
            let Some(id) = run_id.borrow().clone() else {
                return;
            };
            ui2.say("Stopping the test run…");
            rt::spawn(async move { service::cancel(&id).await }, |_| {});
        });
    }

    // ----- Saving and closing -----
    let dirty = {
        let (ui2, saved) = (ui.clone(), saved.clone());
        move || serde_json::to_string(&ui2.current()).unwrap_or_default() != *saved.borrow()
    };
    let dirty = Rc::new(dirty);
    {
        let (ui2, saved, on_saved) = (ui.clone(), saved.clone(), on_saved.clone());
        save.connect_clicked(move |save| {
            let workflow = ui2.current();
            if ui2.invalid(&workflow) {
                return;
            }
            save.set_sensitive(false);
            ui2.say("Saving…");
            let home = ui2.state.borrow().home.clone();
            let (ui3, saved, on_saved, save) =
                (ui2.clone(), saved.clone(), on_saved.clone(), save.clone());
            let json = serde_json::to_string(&workflow).unwrap_or_default();
            let name = workflow.name.clone();
            rt::spawn(
                async move {
                    if workflow.owner == Owner::HomeAssistant {
                        crate::ha_automation::save(&home.base, &home.token, &workflow).await?;
                    }
                    service::save(&workflow).await
                },
                move |result| {
                    save.set_sensitive(true);
                    match result {
                        Ok(()) => {
                            *saved.borrow_mut() = json;
                            ui3.window.set_title(Some(&format!("{name} — Canvas")));
                            ui3.say("Saved.");
                            on_saved();
                        }
                        Err(e) => ui3.say(&e),
                    }
                },
            );
        });
    }
    {
        let (open, dirty, save) = (open.clone(), dirty.clone(), save.clone());
        let confirmed = Rc::new(Cell::new(false));
        window.connect_close_request(move |window| {
            if confirmed.get() || !dirty() {
                open.set(false);
                return glib::Propagation::Proceed;
            }
            let (dialog, card) = widgets::dialog("Save changes?", 400);
            let note = widgets::label(
                "This automation has changes that haven't been saved.",
                "dim",
            );
            note.set_wrap(true);
            card.append(&note);
            let buttons = widgets::hbox(8);
            buttons.set_halign(gtk::Align::End);
            let discard = gtk::Button::with_label("Discard");
            discard.add_css_class("destructive-action");
            let keep = gtk::Button::with_label("Keep editing");
            let save_close = gtk::Button::with_label("Save");
            save_close.add_css_class("suggested-action");
            buttons.append(&discard);
            buttons.append(&keep);
            buttons.append(&save_close);
            card.append(&buttons);
            dialog.set_transient_for(Some(window));
            let (d, w, c) = (dialog.clone(), window.clone(), confirmed.clone());
            discard.connect_clicked(move |_| {
                c.set(true);
                d.close();
                w.close();
            });
            let d = dialog.clone();
            keep.connect_clicked(move |_| d.close());
            let (d, save) = (dialog.clone(), save.clone());
            save_close.connect_clicked(move |_| {
                d.close();
                save.emit_clicked();
            });
            dialog.present();
            keep.grab_focus();
            glib::Propagation::Stop
        });
    }
    window.present();
    area.grab_focus();
}
