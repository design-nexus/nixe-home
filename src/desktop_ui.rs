use gtk::prelude::*;
use nixe_home::automation::{BUS, Edge, INTERFACE, Node, NodeKind, PATH, Workflow};

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
        "RunWorkflow" => proxy
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

fn request(page: gtk::Box, section: &'static str, method: &'static str, args: Option<Vec<String>>) {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(call(method, args));
    });
    gtk::glib::MainContext::default().spawn_local(async move {
        match rx.recv().await {
            Ok(Ok(_)) => refresh(page, section),
            Ok(Err(e)) => {
                let message = label(&e);
                message.add_css_class("home-error");
                page.append(&message);
            }
            Err(_) => {}
        }
    });
}

pub fn refresh(page: gtk::Box, section: &'static str) {
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
                "automations" => render_workflows(&page, &data),
                _ => render_history(&page, &data),
            },
            Ok(Err(e)) => {
                let box_ = card();
                box_.append(&label(&format!("Desktop service unavailable: {e}")));
                box_.append(&label("Check the nixe-homed user service."));
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

fn render_workflows(page: &gtk::Box, data: &str) {
    let workflows: Vec<Workflow> = serde_json::from_str(data).unwrap_or_default();
    let create = card();
    create.append(&label(
        "Quick automation · When a window opens, send a notification",
    ));
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let class = gtk::Entry::builder()
        .placeholder_text("Window class, e.g. firefox")
        .hexpand(true)
        .build();
    let title = gtk::Entry::builder()
        .placeholder_text("Notification title")
        .hexpand(true)
        .build();
    let add = gtk::Button::with_label("Create");
    row.append(&class);
    row.append(&title);
    row.append(&add);
    create.append(&row);
    page.append(&create);
    let page_copy = page.clone();
    add.connect_clicked(move |_| {
        let class_value = class.text().trim().to_string();
        let title_value = title.text().trim().to_string();
        if class_value.is_empty() || title_value.is_empty() {
            return;
        }
        let workflow = Workflow {
            schema_version: 1,
            id: format!("home-{}", uuid::Uuid::new_v4().simple()),
            name: format!("{} opened", class_value),
            enabled: true,
            nodes: vec![
                Node {
                    id: "when".into(),
                    kind: NodeKind::WindowOpened {
                        class: class_value.clone(),
                    },
                },
                Node {
                    id: "then".into(),
                    kind: NodeKind::Notify {
                        title: title_value,
                        body: format!("{} opened", class_value),
                    },
                },
            ],
            edges: vec![Edge {
                from: "when".into(),
                to: "then".into(),
            }],
        };
        if let Ok(json) = serde_json::to_string(&workflow) {
            request(
                page_copy.clone(),
                "automations",
                "SaveWorkflow",
                Some(vec![json]),
            );
        }
    });
    if workflows.is_empty() {
        page.append(&label("No desktop automations yet."));
    }
    for workflow in workflows {
        let box_ = card();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = label(&workflow.name);
        name.set_hexpand(true);
        row.append(&name);
        let run = gtk::Button::with_label("Run");
        let toggle = gtk::Button::with_label(if workflow.enabled {
            "Disable"
        } else {
            "Enable"
        });
        run.set_sensitive(workflow.enabled);
        row.append(&run);
        row.append(&toggle);
        box_.append(&row);
        box_.append(&label(&format!(
            "{} · {} actions",
            if workflow.enabled {
                "Enabled"
            } else {
                "Disabled"
            },
            workflow.nodes.len() - 1
        )));
        page.append(&box_);
        let id = workflow.id.clone();
        let page_copy = page.clone();
        run.connect_clicked(move |_| {
            request(
                page_copy.clone(),
                "automations",
                "RunWorkflow",
                Some(vec![id.clone()]),
            )
        });
        let id = workflow.id;
        let enabled = !workflow.enabled;
        let page_copy = page.clone();
        toggle.connect_clicked(move |_| {
            request(
                page_copy.clone(),
                "automations",
                "SetEnabled",
                Some(vec![id.clone(), enabled.to_string()]),
            )
        });
    }
}

fn render_history(page: &gtk::Box, data: &str) {
    let runs: Vec<serde_json::Value> = serde_json::from_str(data).unwrap_or_default();
    if runs.is_empty() {
        page.append(&label("No automation runs yet."));
    }
    for run in runs {
        let box_ = card();
        box_.append(&label(&format!(
            "{} · {} · {}",
            run["workflow_id"].as_str().unwrap_or(""),
            run["status"].as_str().unwrap_or(""),
            run["trigger"].as_str().unwrap_or("")
        )));
        box_.append(&label(run["detail"].as_str().unwrap_or("")));
        page.append(&box_);
    }
}
