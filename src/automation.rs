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
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
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
    Notify { title: String, body: String },
    SwitchWorkspace { number: u8 },
    SetVolume { percent: u8 },
    LaunchApplication { desktop_id: String },
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
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Edge {
    pub from: String,
    pub to: String,
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
            return Err("Phase 2 workflows need one trigger followed by 1–63 actions".into());
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
        let mut next = HashMap::new();
        let mut incoming = HashSet::new();
        for edge in &self.edges {
            if !ids.contains(edge.from.as_str())
                || !ids.contains(edge.to.as_str())
                || next.insert(edge.from.as_str(), edge.to.as_str()).is_some()
                || !incoming.insert(edge.to.as_str())
            {
                return Err("Edges must form one connected path".into());
            }
            graph.add_edge(edge.from.as_str(), edge.to.as_str(), ());
        }
        if is_cyclic_directed(&graph) || incoming.contains(trigger.as_str()) {
            return Err("Workflow contains a cycle or edge into its trigger".into());
        }
        let mut order = vec![trigger.clone()];
        while let Some(to) = next.get(order.last().unwrap().as_str()) {
            order.push((*to).to_string());
        }
        if order.len() != self.nodes.len()
            || order
                .iter()
                .skip(1)
                .any(|id| kinds[id.as_str()].is_trigger())
        {
            return Err("All actions must follow the trigger".into());
        }
        Ok(order)
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
            }],
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
}
