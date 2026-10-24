use crate::builder::HomeContext;
use gtk::{cairo, prelude::*};
use nixe_home::automation::{
    ActionPolicy, BUS, DryRunContext, Edge, Group, INTERFACE, Node, NodeKind, Owner, PATH, Point,
    Workflow,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Duration,
};

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
    "Repeat",
    "Wait for window",
    "Wait for entity",
    "Notification",
    "Workspace",
    "Volume",
    "Launch application",
    "Entity state",
    "Entity is?",
    "Turn entity on/off",
    "Activate scene",
    "Set variable",
    "Variable equals?",
    "Stop",
];

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
    undo: Vec<Workflow>,
    redo: Vec<Workflow>,
    dry_results: HashMap<String, String>,
    live_details: HashMap<String, String>,
    home_entities: Vec<(String, String, String)>,
}

fn delete_selection(state: &mut CanvasState) {
    if state.selection.is_empty() {
        return;
    }
    snapshot(state);
    state
        .workflow
        .nodes
        .retain(|n| !state.selection.contains(&n.id));
    state
        .workflow
        .edges
        .retain(|e| !state.selection.contains(&e.from) && !state.selection.contains(&e.to));
    state
        .workflow
        .positions
        .retain(|id, _| !state.selection.contains(id));
    state
        .workflow
        .policies
        .retain(|id, _| !state.selection.contains(id));
    for group in &mut state.workflow.groups {
        group.members.retain(|id| !state.selection.contains(id));
    }
    state
        .workflow
        .groups
        .retain(|group| group.members.len() >= 2);
    state.selection.clear();
    state.selected = None;
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

fn add_node(state: &mut CanvasState, name: &str, point: Point) -> bool {
    if state.workflow.nodes.len() >= 64 {
        return false;
    }
    let default = match name {
        "Interval trigger" => "60",
        "Delay" => "1",
        "Workspace" => "1",
        "Volume" => "50",
        "Notification" => "Home | Automation ran",
        "Launch application" => "org.example.App.desktop",
        "Set variable" | "Variable equals?" => "value | text",
        "Repeat" => "2",
        "Wait for window" => "Game | 60",
        "Wait for entity" => "binary_sensor.example | on | 60",
        "Entity state" | "Entity is?" => "binary_sensor.example | on",
        "Turn entity on/off" => "light.example | on",
        "Activate scene" => "scene.example",
        _ => "",
    };
    let Ok(kind) = make_kind(name, default) else {
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
            x: (point.x / 12.0).round() * 12.0,
            y: (point.y / 12.0).round() * 12.0,
        },
    );
    state.selection.clear();
    state.selection.insert(id.clone());
    state.selected = Some(id);
    true
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

fn collapsed_group<'a>(
    workflow: &'a Workflow,
    id: &str,
) -> Option<&'a nixe_home::automation::Group> {
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
        } => format!("{entity_id} | {state} | {timeout_seconds}"),
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
        "Repeat" => NodeKind::Repeat {
            times: value.parse().map_err(|_| "Enter repeat count")?,
        },
        "Wait for window" => {
            let (class, seconds) = value.split_once('|').unwrap_or((value, "60"));
            NodeKind::WaitWindowOpened {
                class: class.trim().into(),
                timeout_seconds: seconds.trim().parse().map_err(|_| "Enter wait timeout")?,
            }
        }
        "Wait for entity" => {
            let mut parts = value.split('|').map(str::trim);
            NodeKind::WaitEntityState {
                entity_id: parts.next().unwrap_or("").into(),
                state: parts.next().unwrap_or("").into(),
                timeout_seconds: parts
                    .next()
                    .unwrap_or("60")
                    .parse()
                    .map_err(|_| "Enter wait timeout")?,
            }
        }
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
        "Entity state" => {
            let (entity_id, state) = value.split_once('|').unwrap_or((value, ""));
            NodeKind::EntityState {
                entity_id: entity_id.trim().into(),
                state: state.trim().into(),
            }
        }
        "Entity is?" => {
            let (entity_id, state) = value.split_once('|').unwrap_or((value, ""));
            NodeKind::EntityIs {
                entity_id: entity_id.trim().into(),
                state: state.trim().into(),
            }
        }
        "Turn entity on/off" => {
            let (entity_id, state) = value.split_once('|').unwrap_or((value, "on"));
            NodeKind::SetEntity {
                entity_id: entity_id.trim().into(),
                turn_on: state.trim() != "off",
            }
        }
        "Activate scene" => NodeKind::ActivateScene {
            entity_id: value.into(),
        },
        "Set variable" | "Variable equals?" => {
            let (variable, contents) = value.split_once('|').unwrap_or((value, ""));
            if variable.trim().is_empty() {
                return Err("Enter variable name | value".into());
            }
            if name == "Variable equals?" {
                NodeKind::VariableEquals {
                    name: variable.trim().into(),
                    value: contents.trim().into(),
                }
            } else {
                NodeKind::SetVariable {
                    name: variable.trim().into(),
                    value: contents.trim().into(),
                }
            }
        }
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
    state.live_details.clear();
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
            if collapsed_group(&state.workflow, &n.id).is_some() {
                return false;
            }
            state
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
    for group in &state.workflow.groups {
        let Some((gx, gy, gx1, gy1)) = group_bounds(&state.workflow, &group.members) else {
            continue;
        };
        cr.set_source_rgba(0.24, 0.40, 0.51, 0.30);
        cr.rectangle(
            gx,
            gy,
            if group.collapsed { NODE_W } else { gx1 - gx },
            if group.collapsed { NODE_H } else { gy1 - gy },
        );
        let _ = cr.fill_preserve();
        cr.set_source_rgba(0.43, 0.70, 0.83, 0.75);
        cr.set_line_width(1.5);
        let _ = cr.stroke();
        cr.set_source_rgb(0.90, 0.94, 0.96);
        cr.select_font_face("Sans", cairo::FontSlant::Normal, cairo::FontWeight::Bold);
        cr.set_font_size(12.0);
        cr.move_to(gx + 10.0, gy + 18.0);
        let _ = cr.show_text(&group.name);
        if group.collapsed {
            cr.move_to(gx + 10.0, gy + 44.0);
            let _ = cr.show_text(&format!("{} nodes", group.members.len()));
        }
    }
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
        if collapsed_group(&state.workflow, &node.id).is_some() {
            continue;
        }
        let Some(p) = state.workflow.positions.get(&node.id) else {
            continue;
        };
        let selected = state.selection.contains(&node.id);
        cr.set_source_rgb(
            if selected { 0.22 } else { 0.16 },
            if selected { 0.31 } else { 0.18 },
            if selected { 0.38 } else { 0.22 },
        );
        cr.rectangle(p.x, p.y, NODE_W, NODE_H);
        let _ = cr.fill_preserve();
        match state.dry_results.get(&node.id).map(String::as_str) {
            Some("skipped") => cr.set_source_rgb(0.40, 0.43, 0.46),
            Some("failed") => cr.set_source_rgb(0.90, 0.34, 0.34),
            Some("running") => cr.set_source_rgb(0.36, 0.68, 0.96),
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
    if let Some(result) = state.borrow().dry_results.get(&id) {
        let status = gtk::Label::new(Some(&format!("Run: {result}")));
        status.set_halign(gtk::Align::Start);
        panel.append(&status);
    }
    if let Some(detail) = state.borrow().live_details.get(&id) {
        let label = gtk::Label::new(Some(detail));
        label.set_halign(gtk::Align::Start);
        label.set_wrap(true);
        panel.append(&label);
    }
    if state.borrow().selection.len() >= 2 {
        let group_button = gtk::Button::with_label("Group selected nodes");
        panel.append(&group_button);
        let s = state.clone();
        let a = area.clone();
        let p = panel.clone();
        let f = feedback.clone();
        group_button.connect_clicked(move |_| {
            let mut state = s.borrow_mut();
            if state
                .selection
                .iter()
                .any(|id| state.workflow.groups.iter().any(|g| g.members.contains(id)))
            {
                f.set_text("Remove existing groups before regrouping");
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
            drop(state);
            a.queue_draw();
            inspector(s.clone(), p.clone(), a.clone(), f.clone());
        });
    }
    if let Some(group_id) = state
        .borrow()
        .workflow
        .groups
        .iter()
        .find(|g| g.members.contains(&id))
        .map(|g| g.id.clone())
    {
        let group = state
            .borrow()
            .workflow
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .unwrap()
            .clone();
        let group_name = gtk::Entry::new();
        group_name.set_text(&group.name);
        group_name.set_placeholder_text(Some("Group name"));
        panel.append(&group_name);
        let rename = gtk::Button::with_label("Rename group");
        panel.append(&rename);
        let s = state.clone();
        let a = area.clone();
        let f = feedback.clone();
        let id_for_rename = group_id.clone();
        rename.connect_clicked(move |_| {
            let name = group_name.text().trim().to_string();
            if name.is_empty() || name.len() > 80 {
                f.set_text("Enter a group name up to 80 characters");
                return;
            }
            let mut state = s.borrow_mut();
            snapshot(&mut state);
            if let Some(group) = state
                .workflow
                .groups
                .iter_mut()
                .find(|g| g.id == id_for_rename)
            {
                group.name = name;
            }
            a.queue_draw();
        });
        let collapse = gtk::Button::with_label(if group.collapsed {
            "Expand group"
        } else {
            "Collapse group"
        });
        panel.append(&collapse);
        let s = state.clone();
        let a = area.clone();
        let p = panel.clone();
        let f = feedback.clone();
        let id_for_toggle = group_id.clone();
        collapse.connect_clicked(move |_| {
            let mut state = s.borrow_mut();
            snapshot(&mut state);
            if let Some(group) = state
                .workflow
                .groups
                .iter_mut()
                .find(|g| g.id == id_for_toggle)
            {
                group.collapsed = !group.collapsed;
            }
            drop(state);
            a.queue_draw();
            inspector(s.clone(), p.clone(), a.clone(), f.clone());
        });
        let ungroup = gtk::Button::with_label("Ungroup");
        panel.append(&ungroup);
        let s = state.clone();
        let a = area.clone();
        let p = panel.clone();
        let f = feedback.clone();
        ungroup.connect_clicked(move |_| {
            let mut state = s.borrow_mut();
            snapshot(&mut state);
            state.workflow.groups.retain(|g| g.id != group_id);
            drop(state);
            a.queue_draw();
            inspector(s.clone(), p.clone(), a.clone(), f.clone());
        });
    }
    let chooser = gtk::DropDown::from_strings(KINDS);
    if let Some(index) = KINDS.iter().position(|entry| *entry == kind) {
        chooser.set_selected(index as u32);
    }
    panel.append(&chooser);
    let input = gtk::Entry::new();
    let is_home = matches!(
        kind,
        "Entity state" | "Entity is?" | "Turn entity on/off" | "Activate scene" | "Wait for entity"
    );
    chooser.set_sensitive(!is_home);
    let (initial_entity, initial_value) = value.split_once('|').unwrap_or((&value, ""));
    input.set_text(if is_home {
        initial_value.trim()
    } else {
        &value
    });
    input.set_placeholder_text(Some("Value, or title | body"));
    panel.append(&input);
    let entity_options: Vec<_> = if is_home {
        state
            .borrow()
            .home_entities
            .iter()
            .filter(|(entity, _, _)| match kind {
                "Activate scene" => entity.starts_with("scene."),
                "Turn entity on/off" => ["light.", "switch.", "fan.", "input_boolean."]
                    .iter()
                    .any(|prefix| entity.starts_with(prefix)),
                _ => true,
            })
            .cloned()
            .collect()
    } else {
        Vec::new()
    };
    let labels: Vec<_> = entity_options
        .iter()
        .map(|(id, name, _)| format!("{name} · {id}"))
        .collect();
    let label_refs: Vec<_> = labels.iter().map(String::as_str).collect();
    let entity_picker = gtk::DropDown::from_strings(&label_refs);
    entity_picker.set_enable_search(true);
    entity_picker.set_selected(
        entity_options
            .iter()
            .position(|(id, _, _)| id == initial_entity.trim())
            .map_or(gtk::INVALID_LIST_POSITION, |i| i as u32),
    );
    if is_home {
        panel.append(&entity_picker);
    }
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
        if !is_home
            && matches!(
                *name,
                "Entity state"
                    | "Entity is?"
                    | "Turn entity on/off"
                    | "Activate scene"
                    | "Wait for entity"
            )
        {
            f.set_text("Add a Home Assistant node to choose an entity");
            return;
        }
        let selected_value = if is_home {
            let Some((entity, _, _)) = entity_options.get(entity_picker.selected() as usize) else {
                f.set_text("Choose a Home Assistant entity");
                return;
            };
            if *name == "Activate scene" {
                entity.clone()
            } else {
                format!("{} | {}", entity, input.text())
            }
        } else {
            input.text().to_string()
        };
        match make_kind(name, &selected_value) {
            Ok(kind) => {
                let mut s = s.borrow_mut();
                snapshot(&mut s);
                if !kind.is_action()
                    || (!kind.supports_retry()
                        && s.workflow
                            .policies
                            .get(&apply_id)
                            .is_some_and(|p| p.retries > 0))
                {
                    s.workflow.policies.remove(&apply_id);
                }
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
    if state.borrow().workflow.owner != Owner::HomeAssistant
        && state
            .borrow()
            .workflow
            .nodes
            .iter()
            .any(|n| n.id == id && n.kind.is_action())
    {
        let current = state
            .borrow()
            .workflow
            .policies
            .get(&id)
            .cloned()
            .unwrap_or(ActionPolicy {
                retries: 0,
                timeout_seconds: 10,
            });
        let timeout = gtk::SpinButton::with_range(1.0, 60.0, 1.0);
        timeout.set_value(current.timeout_seconds as f64);
        let retries = gtk::SpinButton::with_range(0.0, 3.0, 1.0);
        retries.set_value(current.retries as f64);
        let retry_supported = state
            .borrow()
            .workflow
            .nodes
            .iter()
            .find(|n| n.id == id)
            .is_some_and(|n| n.kind.supports_retry());
        retries.set_sensitive(retry_supported);
        panel.append(&gtk::Label::new(Some("Timeout (seconds)")));
        panel.append(&timeout);
        panel.append(&gtk::Label::new(Some("Retries (repeatable actions only)")));
        panel.append(&retries);
        let apply_policy = gtk::Button::with_label("Apply action policy");
        panel.append(&apply_policy);
        let s = state.clone();
        let f = feedback.clone();
        let policy_id = id.clone();
        apply_policy.connect_clicked(move |_| {
            let mut state = s.borrow_mut();
            snapshot(&mut state);
            state.workflow.policies.insert(
                policy_id.clone(),
                ActionPolicy {
                    timeout_seconds: timeout.value() as u64,
                    retries: retries.value() as u8,
                },
            );
            f.set_text("Action policy updated");
        });
    }
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
        s.borrow_mut().connecting = selected.map(|id| (id, None));
        f.set_text("Click a target node on the canvas");
    });
    if state
        .borrow()
        .workflow
        .nodes
        .iter()
        .any(|n| n.id == id && n.kind.is_action())
    {
        let error = gtk::Button::with_label("Connect error to node…");
        panel.append(&error);
        let s = state.clone();
        let f = feedback.clone();
        error.connect_clicked(move |_| {
            let selected = s.borrow().selected.clone();
            s.borrow_mut().connecting = selected.map(|id| (id, Some("error".into())));
            f.set_text("Click the error handler node");
        });
    }
    let remove = gtk::Button::with_label("Remove node");
    panel.append(&remove);
    let s = state.clone();
    let a = area.clone();
    let p = panel.clone();
    let f = feedback.clone();
    remove.connect_clicked(move |_| {
        delete_selection(&mut s.borrow_mut());
        f.set_text("Node removed; reconnect any orphaned nodes");
        a.queue_draw();
        inspector(s.clone(), p.clone(), a.clone(), f.clone());
    });
}

fn daemon_call(method: &str, argument: String) -> Result<String, String> {
    let conn = zbus::blocking::Connection::session()
        .map_err(|_| "Desktop service unavailable".to_string())?;
    let proxy =
        zbus::blocking::Proxy::new(&conn, BUS, PATH, INTERFACE).map_err(|e| e.to_string())?;
    if method == "CancelRun" {
        let canceled: bool = proxy.call(method, &argument).map_err(|e| e.to_string())?;
        Ok(canceled.to_string())
    } else {
        proxy.call(method, &argument).map_err(|e| e.to_string())
    }
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
        selection: HashSet::new(),
        clipboard: None,
        connecting: None,
        pan: (0.0, 0.0),
        zoom: 1.0,
        undo: vec![],
        redo: vec![],
        dry_results: HashMap::new(),
        live_details: HashMap::new(),
        home_entities: home.entities.clone(),
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
    let owner_select =
        gtk::DropDown::from_strings(&["Desktop", "Home Assistant", "Hybrid · desktop-owned"]);
    owner_select.set_selected(match state.borrow().workflow.owner {
        Owner::Desktop => 0,
        Owner::HomeAssistant => 1,
        Owner::HybridDesktop => 2,
    });
    owner_select.set_sensitive(state.borrow().workflow.owner != Owner::HomeAssistant);
    toolbar.append(&owner_select);
    let owner_state = state.clone();
    owner_select.connect_selected_notify(move |drop| {
        let owner = match drop.selected() {
            0 => Owner::Desktop,
            1 => Owner::HomeAssistant,
            _ => Owner::HybridDesktop,
        };
        let mut state = owner_state.borrow_mut();
        if state.workflow.owner != owner {
            snapshot(&mut state);
            state.workflow.owner = owner;
        }
    });
    let add = gtk::Button::with_label("Add node");
    let auto = gtk::Button::with_label("Auto layout");
    let undo = gtk::Button::with_label("Undo");
    let redo = gtk::Button::with_label("Redo");
    let copy = gtk::Button::with_label("Copy");
    let paste = gtk::Button::with_label("Paste");
    let dry = gtk::Button::with_label("Dry run");
    let live = gtk::Button::with_label("Live test");
    live.set_sensitive(state.borrow().workflow.owner != Owner::HomeAssistant);
    let cancel = gtk::Button::with_label("Cancel run");
    cancel.set_sensitive(false);
    let save = gtk::Button::with_label("Save automation");
    save.add_css_class("suggested-action");
    let palette = gtk::DropDown::from_strings(KINDS);
    palette.set_enable_search(true);
    palette.set_selected(12);
    for widget in [
        &add, &auto, &undo, &redo, &copy, &paste, &dry, &live, &cancel, &save,
    ] {
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
    panel.set_margin_start(8);
    panel.set_margin_end(8);
    panel.set_margin_top(8);
    panel.set_margin_bottom(8);
    let inspector_scroll = gtk::ScrolledWindow::new();
    inspector_scroll.set_width_request(260);
    inspector_scroll.set_child(Some(&panel));
    body.append(&area);
    body.append(&inspector_scroll);
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
    click.connect_pressed(move |gesture, _, x, y| {
        let target = hit(&s.borrow(), x, y);
        let source = s.borrow_mut().connecting.take();
        if let (Some((from, requested_port)), Some(to)) = (source, target.clone())
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
            let output = if requested_port.is_some() {
                requested_port
            } else if matches!(from_kind, NodeKind::BranchAppRunning { .. }) {
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
        let shift = gesture
            .current_event_state()
            .contains(gtk::gdk::ModifierType::SHIFT_MASK);
        let mut state = s.borrow_mut();
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
        drop(state);
        a.queue_draw();
        inspector(s.clone(), p.clone(), a.clone(), f.clone());
    });
    area.add_controller(click);
    let drag = gtk::GestureDrag::new();
    let origin = Rc::new(RefCell::new((
        None::<String>,
        HashMap::<String, Point>::new(),
        (0.0, 0.0),
        None::<Workflow>,
    )));
    let s = state.clone();
    let o = origin.clone();
    drag.connect_drag_begin(move |_, x, y| {
        let state = s.borrow();
        let id = hit(&state, x, y);
        let ids: HashSet<String> = if let Some(id) = &id {
            if state.selection.contains(id) {
                state.selection.clone()
            } else {
                HashSet::from([id.clone()])
            }
        } else {
            HashSet::new()
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
    let s = state.clone();
    let o = origin.clone();
    let a = area.clone();
    drag.connect_drag_update(move |_, dx, dy| {
        let origin = o.borrow();
        let mut state = s.borrow_mut();
        if origin.0.is_some() {
            let zoom = state.zoom;
            for (id, start) in &origin.1 {
                if let Some(p) = state.workflow.positions.get_mut(id) {
                    p.x = ((start.x + dx / zoom) / 12.0).round() * 12.0;
                    p.y = ((start.y + dy / zoom) / 12.0).round() * 12.0;
                }
            }
        } else {
            state.pan = (origin.2.0 + dx, origin.2.1 + dy);
        }
        a.queue_draw();
    });
    let s = state.clone();
    let o = origin.clone();
    drag.connect_drag_end(move |_, dx, dy| {
        if (dx.abs() > 2.0 || dy.abs() > 2.0)
            && o.borrow().0.is_some()
            && let Some(before) = o.borrow_mut().3.take()
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
    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gtk::gdk::DragAction::COPY);
    let palette_for_drag = palette.clone();
    drag_source.connect_prepare(move |_, _, _| {
        let name = KINDS.get(palette_for_drag.selected() as usize)?;
        Some(gtk::gdk::ContentProvider::for_value(
            &format!("nixe-home-node:{name}").to_value(),
        ))
    });
    add.add_controller(drag_source);
    let drop_target = gtk::DropTarget::new(String::static_type(), gtk::gdk::DragAction::COPY);
    let s = state.clone();
    let a = area.clone();
    let p = panel.clone();
    let f = feedback.clone();
    drop_target.connect_drop(move |_, value, x, y| {
        let Ok(text) = value.get::<String>() else {
            return false;
        };
        let Some(name) = text.strip_prefix("nixe-home-node:") else {
            return false;
        };
        let (px, py, zoom) = {
            let state = s.borrow();
            (state.pan.0, state.pan.1, state.zoom)
        };
        if !add_node(
            &mut s.borrow_mut(),
            name,
            Point {
                x: (x - px) / zoom,
                y: (y - py) / zoom,
            },
        ) {
            f.set_text("Could not add node");
            return false;
        }
        f.set_text("Node added; connect it to the graph");
        a.queue_draw();
        inspector(s.clone(), p.clone(), a.clone(), f.clone());
        true
    });
    area.add_controller(drop_target);
    let s = state.clone();
    let a = area.clone();
    let p = panel.clone();
    let f = feedback.clone();
    add.connect_clicked(move |_| {
        let Some(name) = KINDS.get(palette.selected() as usize) else {
            return;
        };
        let mut state = s.borrow_mut();
        let count = state.workflow.nodes.len();
        if !add_node(
            &mut state,
            name,
            Point {
                x: 60.0 + (count % 4) as f64 * 230.0,
                y: 80.0 + (count / 4) as f64 * 130.0,
            },
        ) {
            f.set_text("Could not add node");
            return;
        }
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
            state.selection.clear();
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
            state.selection.clear();
        }
        drop(state);
        a.queue_draw();
        inspector(s.clone(), p.clone(), a.clone(), f.clone());
    });
    let s = state.clone();
    let f = feedback.clone();
    copy.connect_clicked(move |_| {
        copy_selection(&mut s.borrow_mut());
        f.set_text("Selected nodes copied");
    });
    let s = state.clone();
    let a = area.clone();
    let p = panel.clone();
    let f = feedback.clone();
    paste.connect_clicked(move |_| {
        if paste_selection(&mut s.borrow_mut()) {
            f.set_text("Nodes pasted; connect them to the graph");
            a.queue_draw();
            inspector(s.clone(), p.clone(), a.clone(), f.clone());
        } else {
            f.set_text("Nothing to paste, or graph is full");
        }
    });
    let keys = gtk::EventControllerKey::new();
    let s = state.clone();
    let a = area.clone();
    let p = panel.clone();
    let f = feedback.clone();
    let key_window = window.clone();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if gtk::prelude::GtkWindowExt::focus(&key_window)
            .is_some_and(|widget| widget.is::<gtk::Entry>() || widget.is::<gtk::SpinButton>())
        {
            return gtk::glib::Propagation::Proceed;
        }
        let ctrl = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        let changed = if ctrl && key == gtk::gdk::Key::a {
            let mut state = s.borrow_mut();
            state.selection = state.workflow.nodes.iter().map(|n| n.id.clone()).collect();
            state.selected = state.workflow.nodes.first().map(|n| n.id.clone());
            true
        } else if ctrl && key == gtk::gdk::Key::c {
            copy_selection(&mut s.borrow_mut());
            true
        } else if ctrl && key == gtk::gdk::Key::v {
            paste_selection(&mut s.borrow_mut())
        } else if ctrl && key == gtk::gdk::Key::z {
            let mut state = s.borrow_mut();
            if let Some(prev) = state.undo.pop() {
                let current = std::mem::replace(&mut state.workflow, prev);
                state.redo.push(current);
                state.selection.clear();
                state.selected = None;
            }
            true
        } else if ctrl && key == gtk::gdk::Key::y {
            let mut state = s.borrow_mut();
            if let Some(next) = state.redo.pop() {
                let current = std::mem::replace(&mut state.workflow, next);
                state.undo.push(current);
                state.selection.clear();
                state.selected = None;
            }
            true
        } else if key == gtk::gdk::Key::Delete {
            delete_selection(&mut s.borrow_mut());
            true
        } else if key == gtk::gdk::Key::Escape {
            let mut state = s.borrow_mut();
            state.selection.clear();
            state.selected = None;
            state.connecting = None;
            true
        } else {
            false
        };
        if changed {
            f.set_text("Canvas updated");
            a.queue_draw();
            inspector(s.clone(), p.clone(), a.clone(), f.clone());
            gtk::glib::Propagation::Stop
        } else {
            gtk::glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);
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
    let run_id = Rc::new(RefCell::new(None::<String>));
    let s = state.clone();
    let f = feedback.clone();
    let a = area.clone();
    let p = panel.clone();
    let cancel_for_live = cancel.clone();
    let run_id_for_live = run_id.clone();
    let name_for_live = workflow_name.clone();
    live.connect_clicked(move |_| {
        let mut workflow = s.borrow().workflow.clone();
        workflow.name = name_for_live.text().to_string();
        if workflow.owner == Owner::HomeAssistant {
            f.set_text("Home Assistant live tests run from the automation list");
            return;
        }
        if let Err(error) = workflow.validate() {
            f.set_text(&error);
            return;
        }
        let Ok(json) = serde_json::to_string(&workflow) else {
            f.set_text("Could not serialize workflow");
            return;
        };
        f.set_text("Starting live test…");
        let (tx, rx) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = tx.send_blocking(daemon_call("RunDraftWorkflow", json));
        });
        let s = s.clone();
        let f = f.clone();
        let a = a.clone();
        let p = p.clone();
        let cancel = cancel_for_live.clone();
        let current = run_id_for_live.clone();
        gtk::glib::MainContext::default().spawn_local(async move {
            let id = match rx.recv().await {
                Ok(Ok(id)) => id,
                Ok(Err(e)) => {
                    f.set_text(&e);
                    return;
                }
                Err(_) => {
                    f.set_text("Live test failed");
                    return;
                }
            };
            *current.borrow_mut() = Some(id.clone());
            cancel.set_sensitive(true);
            loop {
                let (tx, rx) = async_channel::bounded(1);
                let query_id = id.clone();
                std::thread::spawn(move || {
                    let _ = tx.send_blocking(daemon_call("RunProgress", query_id));
                });
                let progress = match rx.recv().await {
                    Ok(Ok(value)) => value,
                    Ok(Err(e)) => {
                        f.set_text(&e);
                        break;
                    }
                    Err(_) => break,
                };
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&progress) else {
                    break;
                };
                let mut statuses = HashMap::new();
                let mut details = HashMap::new();
                for node in value["nodes"].as_array().into_iter().flatten() {
                    if let (Some(id), Some(status)) =
                        (node["node_id"].as_str(), node["status"].as_str())
                    {
                        statuses.insert(id.to_string(), status.to_string());
                        if let Some(detail) = node["detail"].as_str() {
                            details.insert(id.to_string(), detail.to_string());
                        }
                    }
                }
                {
                    let mut state = s.borrow_mut();
                    state.dry_results = statuses;
                    state.live_details = details;
                }
                a.queue_draw();
                let status = value["status"].as_str().unwrap_or("unknown");
                f.set_text(&format!("Live test: {status}"));
                if status != "running" {
                    break;
                }
                gtk::glib::timeout_future(Duration::from_millis(400)).await;
            }
            *current.borrow_mut() = None;
            cancel.set_sensitive(false);
            inspector(s.clone(), p.clone(), a.clone(), f.clone());
        });
    });
    let f = feedback.clone();
    let current = run_id.clone();
    cancel.connect_clicked(move |_| {
        let Some(id) = current.borrow().clone() else {
            return;
        };
        f.set_text("Canceling run…");
        std::thread::spawn(move || {
            let _ = daemon_call("CancelRun", id);
        });
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
