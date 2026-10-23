use petgraph::{algo::is_cyclic_directed, graphmap::DiGraphMap};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const BUS: &str = "io.github.design_nexus.nixe.Home.Automation";
pub const PATH: &str = "/io/github/design_nexus/nixe/Home/Automation";
pub const INTERFACE: &str = "io.github.design_nexus.nixe.Home.Automation1";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub owner: Owner,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub positions: HashMap<String, Point>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    #[default]
    Desktop,
    HomeAssistant,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Node {
    pub id: String,
    #[serde(flatten)]
    pub kind: NodeKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NodeKind {
    Manual,
    Startup,
    Interval { seconds: u64 },
    WindowOpened { class: String },
    WindowClosed { class: String },
    EntityState { entity_id: String, state: String },
    AppRunning { class: String },
    EntityIs { entity_id: String, state: String },
    Notify { title: String, body: String },
    SwitchWorkspace { number: u8 },
    SetVolume { percent: u8 },
    LaunchApplication { desktop_id: String },
    SetEntity { entity_id: String, turn_on: bool },
    ActivateScene { entity_id: String },
    BranchAppRunning { class: String },
    Parallel,
    Delay { seconds: u64 },
    Stop,
}

impl NodeKind {
    pub fn is_trigger(&self) -> bool {
        matches!(
            self,
            Self::Manual
                | Self::Startup
                | Self::Interval { .. }
                | Self::WindowOpened { .. }
                | Self::WindowClosed { .. }
                | Self::EntityState { .. }
        )
    }
    pub fn is_condition(&self) -> bool {
        matches!(self, Self::AppRunning { .. } | Self::EntityIs { .. })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: String,
    pub to: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DryRunContext {
    #[serde(default)]
    pub entity_states: HashMap<String, String>,
    #[serde(default)]
    pub running_apps: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DryRunStep {
    pub node_id: String,
    pub result: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DryRunReport {
    pub steps: Vec<DryRunStep>,
    pub warnings: Vec<String>,
}

impl Workflow {
    pub fn validate(&self) -> Result<Vec<String>, String> {
        if self.schema_version != 1 {
            return Err("Unsupported workflow schema version".into());
        }
        if !valid_id(&self.id) || self.name.trim().is_empty() || self.name.len() > 120 {
            return Err("Workflow needs a valid ID and name".into());
        }
        if self.nodes.len() < 2 || self.nodes.len() > 64 || self.edges.len() != self.nodes.len() - 1
        {
            return Err("Workflows need one trigger followed by 1–63 conditions or actions".into());
        }
        let mut ids = HashSet::new();
        let mut kinds = HashMap::new();
        let mut trigger = None;
        for node in &self.nodes {
            if !valid_id(&node.id) || !ids.insert(node.id.as_str()) {
                return Err("Node IDs must be unique and use letters, digits, _ or -".into());
            }
            match &node.kind {
                NodeKind::Interval { seconds } if !(60..=86_400).contains(seconds) => {
                    return Err("Interval must be 60–86400 seconds".into());
                }
                NodeKind::WindowOpened { class } | NodeKind::WindowClosed { class }
                    if class.trim().is_empty() || class.len() > 128 =>
                {
                    return Err("Window class is required".into());
                }
                NodeKind::AppRunning { class } | NodeKind::BranchAppRunning { class }
                    if class.trim().is_empty() || class.len() > 128 =>
                {
                    return Err("Application class is required".into());
                }
                NodeKind::Delay { seconds } if !(1..=3600).contains(seconds) => {
                    return Err("Delay must be 1–3600 seconds".into());
                }
                NodeKind::EntityState { entity_id, state }
                | NodeKind::EntityIs { entity_id, state }
                    if !valid_entity_id(entity_id)
                        || state.trim().is_empty()
                        || state.len() > 128 =>
                {
                    return Err("Entity state needs a valid entity and value".into());
                }
                NodeKind::SetEntity { entity_id, .. }
                    if !valid_entity_id(entity_id)
                        || !matches!(
                            entity_id.split('.').next(),
                            Some("light" | "switch" | "fan" | "input_boolean")
                        ) =>
                {
                    return Err("This entity does not support the selected on/off action".into());
                }
                NodeKind::ActivateScene { entity_id }
                    if !valid_entity_id(entity_id) || !entity_id.starts_with("scene.") =>
                {
                    return Err("A scene entity is required".into());
                }
                NodeKind::Notify { title, body }
                    if title.trim().is_empty() || title.len() > 120 || body.len() > 1000 =>
                {
                    return Err("Notification title or body is too long".into());
                }
                NodeKind::SwitchWorkspace { number } if !(1..=20).contains(number) => {
                    return Err("Workspace must be 1–20".into());
                }
                NodeKind::SetVolume { percent } if *percent > 100 => {
                    return Err("Volume must be 0–100".into());
                }
                NodeKind::LaunchApplication { desktop_id } if !valid_desktop_id(desktop_id) => {
                    return Err("Application must be a valid desktop file ID".into());
                }
                _ => {}
            }
            if node.kind.is_trigger() && trigger.replace(node.id.clone()).is_some() {
                return Err("Only one trigger is supported".into());
            }
            kinds.insert(node.id.as_str(), &node.kind);
        }
        let trigger = trigger.ok_or("A trigger is required")?;
        let mut graph = DiGraphMap::<&str, ()>::new();
        for id in &ids {
            graph.add_node(id);
        }
        let mut next: HashMap<&str, Vec<&Edge>> = HashMap::new();
        let mut incoming = HashSet::new();
        for edge in &self.edges {
            if !ids.contains(edge.from.as_str())
                || !ids.contains(edge.to.as_str())
                || !incoming.insert(edge.to.as_str())
            {
                return Err("Each node needs one incoming edge, except the trigger".into());
            }
            next.entry(&edge.from).or_default().push(edge);
            graph.add_edge(edge.from.as_str(), edge.to.as_str(), ());
        }
        if is_cyclic_directed(&graph) || incoming.contains(trigger.as_str()) {
            return Err("Workflow contains a cycle or edge into its trigger".into());
        }
        let mut order = Vec::new();
        let mut pending = vec![trigger.clone()];
        while let Some(id) = pending.pop() {
            for edge in next.get(id.as_str()).into_iter().flatten().rev() {
                pending.push(edge.to.clone());
            }
            order.push(id);
        }
        if order.len() != self.nodes.len()
            || order
                .iter()
                .skip(1)
                .any(|id| kinds[id.as_str()].is_trigger())
        {
            return Err("All actions must follow the trigger".into());
        }
        let mut actions = 0;
        for (index, id) in order.iter().enumerate() {
            let kind = kinds[id.as_str()];
            let outgoing = next.get(id.as_str()).map_or(&[][..], Vec::as_slice);
            match kind {
                NodeKind::BranchAppRunning { .. }
                    if outgoing.len() == 2
                        && outgoing.iter().any(|e| e.output.as_deref() == Some("true"))
                        && outgoing
                            .iter()
                            .any(|e| e.output.as_deref() == Some("false")) => {}
                NodeKind::BranchAppRunning { .. } => {
                    return Err("Branch needs true and false outputs".into());
                }
                NodeKind::Parallel
                    if outgoing.len() >= 2 && outgoing.iter().all(|e| e.output.is_none()) => {}
                NodeKind::Parallel => return Err("Parallel needs at least two outputs".into()),
                NodeKind::Stop if outgoing.is_empty() => {}
                NodeKind::Stop => return Err("Stop cannot have outgoing edges".into()),
                _ if outgoing.len() <= 1 && outgoing.iter().all(|e| e.output.is_none()) => {}
                _ => return Err("Only branch or parallel nodes may have multiple outputs".into()),
            }
            if index == 0 {
                continue;
            }
            if kind.is_condition() {
                if self.owner == Owner::HomeAssistant && actions > 0 {
                    return Err("Conditions must come before actions".into());
                }
            } else if !matches!(
                kind,
                NodeKind::BranchAppRunning { .. } | NodeKind::Parallel | NodeKind::Stop
            ) {
                actions += 1;
            }
            let allowed = match self.owner {
                Owner::Desktop => matches!(
                    kind,
                    NodeKind::AppRunning { .. }
                        | NodeKind::Notify { .. }
                        | NodeKind::SwitchWorkspace { .. }
                        | NodeKind::SetVolume { .. }
                        | NodeKind::LaunchApplication { .. }
                        | NodeKind::BranchAppRunning { .. }
                        | NodeKind::Parallel
                        | NodeKind::Delay { .. }
                        | NodeKind::Stop
                ),
                Owner::HomeAssistant => matches!(
                    kind,
                    NodeKind::EntityIs { .. }
                        | NodeKind::SetEntity { .. }
                        | NodeKind::ActivateScene { .. }
                ),
            };
            if !allowed {
                return Err("This node cannot run with the selected owner".into());
            }
        }
        if actions == 0 {
            return Err("At least one action is required".into());
        }
        if self.owner == Owner::HomeAssistant && self.edges.iter().any(|e| e.output.is_some()) {
            return Err("Home Assistant workflow must be linear".into());
        }
        if self.positions.iter().any(|(id, p)| {
            !ids.contains(id.as_str())
                || !p.x.is_finite()
                || !p.y.is_finite()
                || p.x.abs() > 100_000.0
                || p.y.abs() > 100_000.0
        }) {
            return Err("Canvas positions are invalid".into());
        }
        let trigger_kind = kinds[trigger.as_str()];
        if (self.owner == Owner::Desktop && matches!(trigger_kind, NodeKind::EntityState { .. }))
            || (self.owner == Owner::HomeAssistant
                && !matches!(trigger_kind, NodeKind::EntityState { .. }))
        {
            return Err("Trigger cannot run with the selected owner".into());
        }
        Ok(order)
    }

    pub fn to_home_assistant(&self) -> Result<serde_json::Value, String> {
        let order = self.validate()?;
        if self.owner != Owner::HomeAssistant {
            return Err("Workflow is desktop-owned".into());
        }
        if self.edges.iter().any(|e| e.output.is_some()) {
            return Err("Home Assistant workflow must be linear".into());
        }
        let mut conditions = Vec::new();
        let mut actions = Vec::new();
        let mut trigger = None;
        for id in order {
            let kind = &self.nodes.iter().find(|n| n.id == id).unwrap().kind;
            match kind {
                NodeKind::EntityState { entity_id, state } => {
                    trigger = Some(
                        serde_json::json!({"trigger":"state","entity_id":entity_id,"to":state}),
                    )
                }
                NodeKind::EntityIs { entity_id, state } => conditions.push(
                    serde_json::json!({"condition":"state","entity_id":entity_id,"state":state}),
                ),
                NodeKind::SetEntity { entity_id, turn_on } => {
                    let service = if *turn_on { "turn_on" } else { "turn_off" };
                    let domain = entity_id.split('.').next().unwrap();
                    actions.push(serde_json::json!({"action":format!("{domain}.{service}"),"target":{"entity_id":entity_id}}));
                }
                NodeKind::ActivateScene { entity_id } => actions.push(
                    serde_json::json!({"action":"scene.turn_on","target":{"entity_id":entity_id}}),
                ),
                _ => return Err("Unsupported Home Assistant node".into()),
            }
        }
        Ok(
            serde_json::json!({"id":self.id,"alias":self.name,"triggers":[trigger.unwrap()],"conditions":conditions,"actions":actions,"mode":"single"}),
        )
    }

    pub fn dry_run(&self, context: &DryRunContext) -> Result<DryRunReport, String> {
        let order = self.validate()?;
        let mut steps = Vec::new();
        let mut warnings = Vec::new();
        let mut active = HashMap::new();
        let trigger_id = order.first().unwrap().clone();
        active.insert(trigger_id, true);
        for id in order {
            let kind = &self.nodes.iter().find(|n| n.id == id).unwrap().kind;
            let is_active = active.get(&id).copied().unwrap_or(false);
            let mut continue_path = is_active;
            let mut chosen = None;
            let result = match kind {
                _ if !is_active => "skipped".into(),
                kind if kind.is_trigger() => "trigger simulated".to_string(),
                NodeKind::AppRunning { class } | NodeKind::BranchAppRunning { class } => {
                    match &context.running_apps {
                        Some(apps) if apps.iter().any(|app| app.eq_ignore_ascii_case(class)) => {
                            if matches!(kind, NodeKind::BranchAppRunning { .. }) {
                                chosen = Some("true");
                            }
                            "condition true".into()
                        }
                        Some(_) => {
                            if matches!(kind, NodeKind::BranchAppRunning { .. }) {
                                chosen = Some("false");
                            } else {
                                continue_path = false;
                            }
                            "condition false".into()
                        }
                        None => {
                            continue_path = false;
                            warnings.push(
                                "Current application list is unavailable in this dry run".into(),
                            );
                            "condition unknown".into()
                        }
                    }
                }
                NodeKind::EntityIs { entity_id, state } => {
                    match context.entity_states.get(entity_id) {
                        Some(actual) if actual == state => "condition true".into(),
                        Some(_) => {
                            continue_path = false;
                            "condition false".into()
                        }
                        None => {
                            continue_path = false;
                            warnings.push(format!("No current state for {entity_id}"));
                            "condition unknown".into()
                        }
                    }
                }
                NodeKind::Parallel => "parallel paths planned".into(),
                NodeKind::Stop => {
                    continue_path = false;
                    "stop".into()
                }
                NodeKind::Delay { seconds } => format!("planned delay: {seconds}s"),
                _ => "planned action".into(),
            };
            for edge in self.edges.iter().filter(|edge| edge.from == id) {
                active.insert(
                    edge.to.clone(),
                    continue_path && chosen.is_none_or(|port| edge.output.as_deref() == Some(port)),
                );
            }
            steps.push(DryRunStep {
                node_id: id,
                result,
            });
        }
        Ok(DryRunReport { steps, warnings })
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn valid_desktop_id(s: &str) -> bool {
    s.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && s.len() <= 128
        && s.ends_with(".desktop")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn valid_entity_id(s: &str) -> bool {
    let Some((domain, object)) = s.split_once('.') else {
        return false;
    };
    !domain.is_empty()
        && !object.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_disconnected_and_unsafe_actions() {
        let mut w = Workflow {
            schema_version: 1,
            id: "test".into(),
            name: "Test".into(),
            enabled: true,
            owner: Owner::Desktop,
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    kind: NodeKind::Manual,
                },
                Node {
                    id: "action".into(),
                    kind: NodeKind::LaunchApplication {
                        desktop_id: "org.example.App.desktop".into(),
                    },
                },
            ],
            edges: vec![Edge {
                from: "trigger".into(),
                to: "action".into(),
                output: None,
            }],
            positions: HashMap::new(),
        };
        let json = serde_json::to_string(&w).unwrap();
        let restored: Workflow = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.validate().unwrap(), ["trigger", "action"]);
        assert!(w.validate().is_ok());
        w.edges[0].to = "trigger".into();
        assert!(w.validate().is_err());
        w.edges[0].to = "action".into();
        w.nodes[1].kind = NodeKind::LaunchApplication {
            desktop_id: "$(touch /tmp/no).desktop".into(),
        };
        assert!(w.validate().is_err());
    }

    #[test]
    fn home_translation_and_dry_run() {
        let w = Workflow {
            schema_version: 1,
            id: "ha-test".into(),
            name: "Motion lights".into(),
            enabled: true,
            owner: Owner::HomeAssistant,
            nodes: vec![
                Node {
                    id: "when".into(),
                    kind: NodeKind::EntityState {
                        entity_id: "binary_sensor.motion".into(),
                        state: "on".into(),
                    },
                },
                Node {
                    id: "if".into(),
                    kind: NodeKind::EntityIs {
                        entity_id: "person.ken".into(),
                        state: "home".into(),
                    },
                },
                Node {
                    id: "then".into(),
                    kind: NodeKind::SetEntity {
                        entity_id: "light.office".into(),
                        turn_on: true,
                    },
                },
            ],
            edges: vec![
                Edge {
                    from: "when".into(),
                    to: "if".into(),
                    output: None,
                },
                Edge {
                    from: "if".into(),
                    to: "then".into(),
                    output: None,
                },
            ],
            positions: HashMap::new(),
        };
        let config = w.to_home_assistant().unwrap();
        assert_eq!(config["actions"][0]["action"], "light.turn_on");
        let report = w
            .dry_run(&DryRunContext {
                entity_states: HashMap::from([("person.ken".into(), "away".into())]),
                running_apps: None,
            })
            .unwrap();
        assert_eq!(report.steps[2].result, "skipped");
    }

    #[test]
    fn branch_paths_validate_and_dry_run() {
        let workflow = Workflow {
            schema_version: 1,
            id: "branch-test".into(),
            name: "Branch".into(),
            enabled: true,
            owner: Owner::Desktop,
            nodes: vec![
                Node {
                    id: "when".into(),
                    kind: NodeKind::Manual,
                },
                Node {
                    id: "branch".into(),
                    kind: NodeKind::BranchAppRunning {
                        class: "Game".into(),
                    },
                },
                Node {
                    id: "yes".into(),
                    kind: NodeKind::Notify {
                        title: "Yes".into(),
                        body: String::new(),
                    },
                },
                Node {
                    id: "no".into(),
                    kind: NodeKind::Notify {
                        title: "No".into(),
                        body: String::new(),
                    },
                },
            ],
            edges: vec![
                Edge {
                    from: "when".into(),
                    to: "branch".into(),
                    output: None,
                },
                Edge {
                    from: "branch".into(),
                    to: "yes".into(),
                    output: Some("true".into()),
                },
                Edge {
                    from: "branch".into(),
                    to: "no".into(),
                    output: Some("false".into()),
                },
            ],
            positions: HashMap::new(),
        };
        let restored: Workflow =
            serde_json::from_str(&serde_json::to_string(&workflow).unwrap()).unwrap();
        restored.validate().unwrap();
        let report = restored
            .dry_run(&DryRunContext {
                running_apps: Some(vec!["Game".into()]),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            report
                .steps
                .iter()
                .find(|s| s.node_id == "yes")
                .unwrap()
                .result,
            "planned action"
        );
        assert_eq!(
            report
                .steps
                .iter()
                .find(|s| s.node_id == "no")
                .unwrap()
                .result,
            "skipped"
        );
        let mut invalid = workflow;
        invalid.edges[2].output = Some("true".into());
        assert!(invalid.validate().is_err());
    }
}
