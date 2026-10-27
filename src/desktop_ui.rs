use crate::{
    builder::{self, HomeContext},
    canvas,
};
use gtk::prelude::*;
use nyx_home::automation::{BUS, Edge, INTERFACE, Node, NodeKind, Owner, PATH, Workflow};
use std::rc::Rc;

pub fn page(section: &str) -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    page.set_valign(gtk::Align::Start);
    page.add_css_class("home-content");
    let heading = gtk::Label::new(Some(match section {
        "system" => "System",
        "automations" => "Automations",
        _ => "Activity",
    }));
    heading.set_halign(gtk::Align::Start);
    heading.add_css_class("home-heading");
    page.append(&heading);
    page
}

fn label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_halign(gtk::Align::Start);
    label.set_wrap(true);
    label
}

fn card() -> gtk::Box {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("home-card");
    card
}

fn clear(page: &gtk::Box) {
    while let Some(child) = page.first_child() {
        page.remove(&child);
    }
}

fn call(method: &str, args: Option<Vec<String>>) -> Result<String, String> {
    let connection = zbus::blocking::Connection::session()
        .map_err(|_| "Desktop service unavailable".to_string())?;
    let proxy =
        zbus::blocking::Proxy::new(&connection, BUS, PATH, INTERFACE).map_err(|e| e.to_string())?;
    match method {
        "SaveWorkflow" => proxy
            .call(method, &(args.unwrap()[0].clone()))
            .map_err(|e| e.to_string()),
        "RunWorkflow" | "RunProgress" => proxy
            .call(method, &(args.unwrap()[0].clone()))
            .map_err(|e| e.to_string()),
        "SetEnabled" => {
            let args = args.unwrap();
            proxy
                .call::<_, _, ()>(method, &(args[0].clone(), args[1] == "true"))
                .map_err(|e| e.to_string())?;
            Ok(String::new())
        }
        "History" => proxy.call(method, &(40u32)).map_err(|e| e.to_string()),
        _ => proxy.call(method, &()).map_err(|e| e.to_string()),
    }
}

fn request(
    page: gtk::Box,
    section: &'static str,
    home: HomeContext,
    method: &'static str,
    args: Option<Vec<String>>,
) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(call(method, args));
    });
    gtk::glib::MainContext::default().spawn_local(async move {
        match rx.recv().await {
            Ok(Ok(_)) => refresh(page, section, home),
            Ok(Err(e)) => {
                let message = label(&e);
                message.add_css_class("home-error");
                page.append(&message);
            }
            Err(_) => {}
        }
    });
}

fn request_home(
    page: gtk::Box,
    home: HomeContext,
    id: String,
    service: &'static str,
    enabled: Option<bool>,
) {
    let (tx, rx) = async_channel::bounded(1);
    let mut next_home = home.clone();
    if let Some(value) = enabled {
        next_home
            .automations
            .insert(id.clone(), if value { "on" } else { "off" }.into());
    }
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| e.to_string())?;
            rt.block_on(crate::ha_automation::control(
                &home.base,
                &home.token,
                &id,
                service,
            ))?;
            if let Some(value) = enabled {
                call("SetEnabled", Some(vec![id, value.to_string()]))?;
            }
            Ok(())
        })();
        let _ = tx.send_blocking(result);
    });
    gtk::glib::MainContext::default().spawn_local(async move {
        match rx.recv().await {
            Ok(Ok(())) => refresh(page, "automations", next_home),
            Ok(Err(e)) => {
                let message = label(&e);
                message.add_css_class("home-error");
                page.append(&message);
            }
            Err(_) => {}
        }
    });
}

pub fn refresh(page: gtk::Box, section: &'static str, home: HomeContext) {
    clear(&page);
    let heading = label(match section {
        "system" => "System",
        "automations" => "Automations",
        _ => "Activity",
    });
    heading.add_css_class("home-heading");
    page.append(&heading);
    let loading = label("Loading desktop service…");
    page.append(&loading);
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let result = call(
            match section {
                "system" => "Status",
                "automations" => "ListWorkflows",
                _ => "History",
            },
            None,
        );
        let _ = tx.send_blocking(result);
    });
    gtk::glib::MainContext::default().spawn_local(async move {
        page.remove(&loading);
        match rx.recv().await {
            Ok(Ok(data)) => match section {
                "system" => render_system(&page, &data),
                "automations" => render_workflows(&page, &data, home),
                _ => render_history(&page, &data),
            },
            Ok(Err(e)) => {
                let box_ = card();
                box_.append(&label(&format!("Desktop service unavailable: {e}")));
                box_.append(&label("Check the nyx-homed user service."));
                page.append(&box_);
            }
            Err(_) => {}
        }
    });
}

fn render_system(page: &gtk::Box, status: &str) {
    let box_ = card();
    box_.append(&label(&format!("Desktop automation service: {status}")));
    box_.append(&label("Available actions: notifications, application launch, workspace switching, and volume. Window open and close events come from Hyprland."));
    page.append(&box_);
}

fn render_workflows(page: &gtk::Box, data: &str, home: HomeContext) {
    let workflows: Vec<Workflow> = serde_json::from_str(data).unwrap_or_default();
    let create = card();
    create.append(&label("Build an automation with WHEN → IF → THEN"));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let desktop = gtk::Button::with_label("Desktop automation");
    let advanced = gtk::Button::with_label("Advanced canvas");
    let home_button = gtk::Button::with_label("Home automation");
    let hybrid_button = gtk::Button::with_label("Hybrid automation");
    home_button.set_sensitive(
        !home.base.is_empty() && !home.token.is_empty() && !home.entities.is_empty(),
    );
    hybrid_button.set_sensitive(
        !home.base.is_empty()
            && home.entities.iter().any(|(id, _, _)| {
                ["light.", "switch.", "fan.", "input_boolean."]
                    .iter()
                    .any(|prefix| id.starts_with(prefix))
            }),
    );
    row.append(&desktop);
    row.append(&advanced);
    row.append(&home_button);
    row.append(&hybrid_button);
    create.append(&row);
    page.append(&create);
    for (button, owner) in [
        (desktop, Owner::Desktop),
        (home_button, Owner::HomeAssistant),
    ] {
        let page = page.clone();
        let home = home.clone();
        button.connect_clicked(move |_| open_builder(&page, None, owner, home.clone()));
    }
    let page_advanced = page.clone();
    let home_advanced = home.clone();
    advanced.connect_clicked(move |_| {
        let workflow = Workflow {
            schema_version: 1,
            id: format!("home-{}", uuid::Uuid::new_v4().simple()),
            name: "New desktop automation".into(),
            enabled: true,
            owner: Owner::Desktop,
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    kind: NodeKind::Manual,
                },
                Node {
                    id: "action".into(),
                    kind: NodeKind::Notify {
                        title: "Home".into(),
                        body: "Automation ran".into(),
                    },
                },
            ],
            edges: vec![Edge {
                from: "trigger".into(),
                to: "action".into(),
                output: None,
            }],
            positions: Default::default(),
            policies: Default::default(),
            groups: Default::default(),
        };
        open_canvas(&page_advanced, workflow, home_advanced.clone());
    });
    let page_hybrid = page.clone();
    let home_hybrid = home.clone();
    hybrid_button.connect_clicked(move |_| {
        let Some(entity_id) = home_hybrid
            .entities
            .iter()
            .find(|(id, _, _)| {
                ["light.", "switch.", "fan.", "input_boolean."]
                    .iter()
                    .any(|prefix| id.starts_with(prefix))
            })
            .map(|(id, _, _)| id.clone())
        else {
            return;
        };
        let workflow = Workflow {
            schema_version: 1,
            id: format!("home-{}", uuid::Uuid::new_v4().simple()),
            name: "New hybrid automation".into(),
            enabled: true,
            owner: Owner::HybridDesktop,
            nodes: vec![
                Node {
                    id: "trigger".into(),
                    kind: NodeKind::Manual,
                },
                Node {
                    id: "action".into(),
                    kind: NodeKind::SetEntity {
                        entity_id,
                        turn_on: true,
                    },
                },
            ],
            edges: vec![Edge {
                from: "trigger".into(),
                to: "action".into(),
                output: None,
            }],
            positions: Default::default(),
            policies: Default::default(),
            groups: Default::default(),
        };
        open_canvas(&page_hybrid, workflow, home_hybrid.clone());
    });
    if workflows.is_empty() {
        page.append(&label("No automations yet."));
    }
    for workflow in workflows {
        let is_enabled = if workflow.owner == Owner::HomeAssistant {
            home.automations
                .get(&workflow.id)
                .map(|state| state == "on")
                .unwrap_or(workflow.enabled)
        } else {
            workflow.enabled
        };
        let box_ = card();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = label(&workflow.name);
        name.set_hexpand(true);
        row.append(&name);
        let run = gtk::Button::with_label("Live test");
        let edit = gtk::Button::with_label("Edit");
        let canvas_button = gtk::Button::with_label("Canvas");
        let toggle = gtk::Button::with_label(if is_enabled { "Disable" } else { "Enable" });
        run.set_sensitive(
            is_enabled && (workflow.owner != Owner::HomeAssistant || !home.token.is_empty()),
        );
        row.append(&run);
        row.append(&edit);
        row.append(&canvas_button);
        row.append(&toggle);
        box_.append(&row);
        box_.append(&label(&format!(
            "{} · {} · {} actions",
            match workflow.owner {
                Owner::Desktop => "Desktop",
                Owner::HomeAssistant => "Home Assistant",
                Owner::HybridDesktop => "Hybrid · desktop-owned",
            },
            if is_enabled { "Enabled" } else { "Disabled" },
            workflow
                .nodes
                .iter()
                .filter(|n| !n.kind.is_trigger() && !n.kind.is_condition())
                .count()
        )));
        page.append(&box_);
        let edit_workflow = workflow.clone();
        let page_copy = page.clone();
        let home_copy = home.clone();
        edit.connect_clicked(move |_| {
            if edit_workflow.owner == Owner::HybridDesktop {
                open_canvas(&page_copy, edit_workflow.clone(), home_copy.clone());
            } else {
                open_builder(
                    &page_copy,
                    Some(edit_workflow.clone()),
                    edit_workflow.owner,
                    home_copy.clone(),
                )
            }
        });
        let page_canvas = page.clone();
        let home_canvas = home.clone();
        let workflow_canvas = workflow.clone();
        canvas_button.connect_clicked(move |_| {
            open_canvas(&page_canvas, workflow_canvas.clone(), home_canvas.clone())
        });
        let id = workflow.id.clone();
        let page_copy = page.clone();
        let home_copy = home.clone();
        let owner = workflow.owner;
        run.connect_clicked(move |_| {
            if owner != Owner::HomeAssistant {
                request(
                    page_copy.clone(),
                    "automations",
                    home_copy.clone(),
                    "RunWorkflow",
                    Some(vec![id.clone()]),
                );
            } else {
                request_home(
                    page_copy.clone(),
                    home_copy.clone(),
                    id.clone(),
                    "trigger",
                    None,
                );
            }
        });
        let id = workflow.id;
        let enabled = !is_enabled;
        let page_copy = page.clone();
        let home_copy = home.clone();
        let owner = workflow.owner;
        toggle.set_sensitive(owner != Owner::HomeAssistant || !home.token.is_empty());
        toggle.connect_clicked(move |_| {
            if owner != Owner::HomeAssistant {
                request(
                    page_copy.clone(),
                    "automations",
                    home_copy.clone(),
                    "SetEnabled",
                    Some(vec![id.clone(), enabled.to_string()]),
                );
            } else {
                request_home(
                    page_copy.clone(),
                    home_copy.clone(),
                    id.clone(),
                    if enabled { "turn_on" } else { "turn_off" },
                    Some(enabled),
                );
            }
        });
    }
}

fn open_canvas(page: &gtk::Box, workflow: Workflow, home: HomeContext) {
    let Some(parent) = page.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
        return;
    };
    let page_copy = page.clone();
    let home_copy = home.clone();
    canvas::present(
        &parent,
        workflow,
        home,
        Rc::new(move || refresh(page_copy.clone(), "automations", home_copy.clone())),
    );
}

fn open_builder(page: &gtk::Box, workflow: Option<Workflow>, owner: Owner, home: HomeContext) {
    let Some(parent) = page.root().and_then(|r| r.downcast::<gtk::Window>().ok()) else {
        return;
    };
    let page_copy = page.clone();
    let home_copy = home.clone();
    builder::present(
        &parent,
        workflow,
        owner,
        home,
        Rc::new(move || refresh(page_copy.clone(), "automations", home_copy.clone())),
    );
}

fn render_history(page: &gtk::Box, data: &str) {
    let runs: Vec<serde_json::Value> = serde_json::from_str(data).unwrap_or_default();
    if runs.is_empty() {
        page.append(&label("No automation runs yet."));
    }
    for run in runs {
        let box_ = card();
        let started = run["started_at"].as_i64().unwrap_or_default();
        let timestamp = gtk::glib::DateTime::from_unix_local(started)
            .ok()
            .and_then(|date| date.format("%x %X").ok())
            .map(|text| text.to_string())
            .unwrap_or_else(|| started.to_string());
        let duration = run["finished_at"]
            .as_i64()
            .map(|end| format!(" · {}s", (end - started).max(0)))
            .unwrap_or_default();
        box_.append(&label(&format!(
            "{} · {} · {} · {}{}",
            run["workflow_id"].as_str().unwrap_or(""),
            run["status"].as_str().unwrap_or(""),
            run["trigger"].as_str().unwrap_or(""),
            timestamp,
            duration
        )));
        box_.append(&label(run["detail"].as_str().unwrap_or("")));
        if let Some(id) = run["id"].as_str() {
            let details = gtk::Button::with_label("Node details");
            details.set_halign(gtk::Align::Start);
            box_.append(&details);
            let id = id.to_string();
            let box_copy = box_.clone();
            details.connect_clicked(move |button| {
                button.set_sensitive(false);
                let (tx, rx) = async_channel::bounded(1);
                let id = id.clone();
                std::thread::spawn(move || {
                    let _ = tx.send_blocking(call("RunProgress", Some(vec![id])));
                });
                let box_copy = box_copy.clone();
                gtk::glib::MainContext::default().spawn_local(async move {
                    match rx.recv().await {
                        Ok(Ok(data)) => {
                            if let Ok(report) = serde_json::from_str::<serde_json::Value>(&data) {
                                for node in report["nodes"].as_array().into_iter().flatten() {
                                    box_copy.append(&label(&format!(
                                        "{} · {} · {}",
                                        node["node_id"].as_str().unwrap_or(""),
                                        node["status"].as_str().unwrap_or(""),
                                        node["detail"].as_str().unwrap_or("")
                                    )));
                                }
                            }
                        }
                        Ok(Err(error)) => box_copy.append(&label(&error)),
                        Err(_) => {}
                    }
                });
            });
        }
        page.append(&box_);
    }
}
