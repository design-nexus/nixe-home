use gtk::prelude::*;
use nixe_home::automation::{
    BUS, DryRunContext, Edge, INTERFACE, Node, NodeKind, Owner, PATH, Workflow,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

#[derive(Clone, Default)]
pub struct HomeContext {
    pub base: String,
    pub token: String,
    pub entities: Vec<(String, String, String)>, // ID, friendly name, state
    pub automations: HashMap<String, String>,    // HA config ID → actual state
}

#[derive(Clone)]
struct EntityPicker {
    widget: gtk::DropDown,
    ids: Vec<String>,
}

impl EntityPicker {
    fn new(home: &HomeContext, filter: impl Fn(&str) -> bool) -> Self {
        let options: Vec<_> = home
            .entities
            .iter()
            .filter(|(id, _, _)| filter(id))
            .map(|(id, name, _)| (id.clone(), format!("{name} · {id}")))
            .collect();
        Self::from_options(options)
    }
    fn applications() -> Self {
        let options = gtk::gio::AppInfo::all()
            .into_iter()
            .filter(|app| app.should_show())
            .filter_map(|app| {
                app.id()
                    .map(|id| (id.to_string(), app.display_name().to_string()))
            })
            .filter(|(id, _)| id.ends_with(".desktop"))
            .collect();
        Self::from_options(options)
    }
    fn from_options(mut options: Vec<(String, String)>) -> Self {
        options.sort_by(|a, b| a.1.cmp(&b.1));
        let ids: Vec<String> = options.iter().map(|(id, _)| id.clone()).collect();
        let labels: Vec<String> = options.iter().map(|(_, name)| name.clone()).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        let widget = gtk::DropDown::from_strings(&refs);
        widget.set_enable_search(true);
        widget.set_hexpand(true);
        widget.set_selected(gtk::INVALID_LIST_POSITION);
        Self { widget, ids }
    }
    fn value(&self) -> String {
        self.ids
            .get(self.widget.selected() as usize)
            .cloned()
            .unwrap_or_default()
    }
    fn set(&self, id: &str) {
        if let Some(index) = self.ids.iter().position(|candidate| candidate == id) {
            self.widget.set_selected(index as u32);
        }
    }
}

#[derive(Clone)]
struct Choice {
    widget: gtk::DropDown,
    ids: Vec<String>,
}
impl Choice {
    fn new(options: &[(&str, &str)]) -> Self {
        let ids = options.iter().map(|(id, _)| (*id).to_string()).collect();
        let names: Vec<_> = options.iter().map(|(_, name)| *name).collect();
        let widget = gtk::DropDown::from_strings(&names);
        Self { widget, ids }
    }
    fn active_id(&self) -> Option<&str> {
        self.ids
            .get(self.widget.selected() as usize)
            .map(String::as_str)
    }
    fn set_active_id(&self, id: &str) {
        if let Some(i) = self.ids.iter().position(|candidate| candidate == id) {
            self.widget.set_selected(i as u32);
        }
    }
}

struct ActionRow {
    container: gtk::Box,
    kind: Choice,
    value: gtk::Entry,
    body: gtk::Entry,
    entity: Option<EntityPicker>,
    app: Option<EntityPicker>,
}

fn row(owner: Owner, home: &HomeContext, initial: Option<&NodeKind>) -> Rc<ActionRow> {
    let container = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let kind = if owner == Owner::Desktop {
        Choice::new(&[
            ("notify", "Send notification"),
            ("workspace", "Switch workspace"),
            ("volume", "Set volume"),
            ("launch", "Launch application"),
        ])
    } else {
        Choice::new(&[
            ("on", "Turn on"),
            ("off", "Turn off"),
            ("scene", "Activate scene"),
        ])
    };
    let value = gtk::Entry::builder()
        .placeholder_text("Title, number, or desktop application")
        .hexpand(true)
        .build();
    let body = gtk::Entry::builder()
        .placeholder_text("Notification message")
        .hexpand(true)
        .build();
    let entity = if owner == Owner::HomeAssistant {
        Some(EntityPicker::new(home, |id| {
            ["light.", "switch.", "fan.", "input_boolean.", "scene."]
                .iter()
                .any(|prefix| id.starts_with(prefix))
        }))
    } else {
        None
    };
    let app = if owner == Owner::Desktop {
        Some(EntityPicker::applications())
    } else {
        None
    };
    container.append(&kind.widget);
    if let Some(picker) = &entity {
        container.append(&picker.widget);
    } else {
        container.append(&value);
        container.append(&body);
        if let Some(picker) = &app {
            container.append(&picker.widget);
            picker.widget.set_visible(false);
        }
    }
    let result = Rc::new(ActionRow {
        container,
        kind,
        value,
        body,
        entity,
        app,
    });
    if let Some(initial) = initial {
        let (kind, value, body) = match initial {
            NodeKind::Notify { title, body } => ("notify", title.clone(), body.clone()),
            NodeKind::SwitchWorkspace { number } => {
                ("workspace", number.to_string(), String::new())
            }
            NodeKind::SetVolume { percent } => ("volume", percent.to_string(), String::new()),
            NodeKind::LaunchApplication { desktop_id } => {
                ("launch", desktop_id.clone(), String::new())
            }
            NodeKind::SetEntity { entity_id, turn_on } => (
                if *turn_on { "on" } else { "off" },
                entity_id.clone(),
                String::new(),
            ),
            NodeKind::ActivateScene { entity_id } => ("scene", entity_id.clone(), String::new()),
            _ => ("", String::new(), String::new()),
        };
        result.kind.set_active_id(kind);
        result.value.set_text(&value);
        result.body.set_text(&body);
        if let Some(picker) = &result.entity {
            picker.set(&value);
        }
        if let Some(picker) = &result.app {
            picker.set(&value);
        }
    }
    if owner == Owner::Desktop {
        let value = result.value.clone();
        let body = result.body.clone();
        let app = result.app.as_ref().map(|picker| picker.widget.clone());
        let ids = result.kind.ids.clone();
        let update = move |index: usize| {
            let kind = ids.get(index).map(String::as_str).unwrap_or("");
            body.set_visible(kind == "notify");
            value.set_visible(kind != "launch");
            if let Some(app) = &app {
                app.set_visible(kind == "launch");
            }
            value.set_placeholder_text(Some(match kind {
                "notify" => "Notification title",
                "workspace" => "Workspace number",
                "volume" => "Volume percent",
                "launch" => "Application desktop file, e.g. firefox.desktop",
                _ => "Value",
            }));
        };
        update(result.kind.widget.selected() as usize);
        let value = result.value.clone();
        let body = result.body.clone();
        let app = result.app.as_ref().map(|picker| picker.widget.clone());
        let ids = result.kind.ids.clone();
        result.kind.widget.connect_selected_notify(move |dropdown| {
            let kind = ids
                .get(dropdown.selected() as usize)
                .map(String::as_str)
                .unwrap_or("");
            body.set_visible(kind == "notify");
            value.set_visible(kind != "launch");
            if let Some(app) = &app {
                app.set_visible(kind == "launch");
            }
            value.set_placeholder_text(Some(match kind {
                "notify" => "Notification title",
                "workspace" => "Workspace number",
                "volume" => "Volume percent",
                "launch" => "Application desktop file, e.g. firefox.desktop",
                _ => "Value",
            }));
        });
    }
    result
}

fn entry(placeholder: &str) -> gtk::Entry {
    gtk::Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .build()
}
fn label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_halign(gtk::Align::Start);
    l
}
fn section(content: &gtk::Box, title: &str, widget: &impl IsA<gtk::Widget>) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 7);
    card.add_css_class("home-card");
    let heading = label(title);
    heading.add_css_class("home-heading");
    card.append(&heading);
    card.append(widget);
    content.append(&card);
}

fn existing_simple(workflow: &Workflow) -> bool {
    workflow.validate().is_ok()
        && workflow
            .nodes
            .iter()
            .filter(|n| n.kind.is_condition())
            .count()
            <= 1
        && workflow.nodes.iter().all(|n| {
            matches!(
                n.kind,
                NodeKind::Manual
                    | NodeKind::Startup
                    | NodeKind::Interval { .. }
                    | NodeKind::WindowOpened { .. }
                    | NodeKind::WindowClosed { .. }
                    | NodeKind::EntityState { .. }
                    | NodeKind::AppRunning { .. }
                    | NodeKind::EntityIs { .. }
                    | NodeKind::Notify { .. }
                    | NodeKind::SwitchWorkspace { .. }
                    | NodeKind::SetVolume { .. }
                    | NodeKind::LaunchApplication { .. }
                    | NodeKind::SetEntity { .. }
                    | NodeKind::ActivateScene { .. }
            )
        })
}

pub fn present(
    parent: &gtk::Window,
    original: Option<Workflow>,
    owner: Owner,
    home: HomeContext,
    on_saved: Rc<dyn Fn()>,
) {
    if let Some(workflow) = &original
        && !existing_simple(workflow)
    {
        let window = gtk::Window::builder()
            .title(&workflow.name)
            .transient_for(parent)
            .modal(true)
            .default_width(420)
            .build();
        let box_ = gtk::Box::new(gtk::Orientation::Vertical, 12);
        box_.set_margin_top(20);
        box_.set_margin_bottom(20);
        box_.set_margin_start(20);
        box_.set_margin_end(20);
        let message = label(
            "This workflow has steps the simple builder cannot represent. Its saved graph has not been changed. Open it in the advanced canvas when that editor is available.",
        );
        message.set_wrap(true);
        box_.append(&message);
        let close = gtk::Button::with_label("Close");
        box_.append(&close);
        let copy = window.clone();
        close.connect_clicked(move |_| copy.close());
        window.set_child(Some(&box_));
        window.present();
        return;
    }
    let dialog = gtk::Window::builder()
        .title(if original.is_some() {
            "Edit automation"
        } else {
            "New automation"
        })
        .transient_for(parent)
        .modal(true)
        .default_width(650)
        .default_height(540)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 10);
    content.set_spacing(10);
    content.set_margin_start(14);
    content.set_margin_end(14);
    content.set_margin_top(14);
    content.set_margin_bottom(14);
    dialog.set_child(Some(&content));
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    let fields = gtk::Box::new(gtk::Orientation::Vertical, 10);
    scroll.set_child(Some(&fields));
    content.append(&scroll);
    let name = entry("Automation name");
    if let Some(w) = &original {
        name.set_text(&w.name);
    }
    section(&fields, "Name", &name);

    let when = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let trigger = if owner == Owner::Desktop {
        Choice::new(&[
            ("manual", "Manual"),
            ("startup", "Desktop starts"),
            ("interval", "Every N minutes"),
            ("opened", "Window opens"),
            ("closed", "Window closes"),
        ])
    } else {
        Choice::new(&[("entity", "Entity changes state")])
    };
    when.append(&trigger.widget);
    let trigger_value = entry(if owner == Owner::Desktop {
        "Minutes or window class"
    } else {
        "Target state, e.g. on"
    });
    let trigger_entity = if owner == Owner::HomeAssistant {
        let picker = EntityPicker::new(&home, |_| true);
        when.append(&picker.widget);
        Some(picker)
    } else {
        None
    };
    when.append(&trigger_value);
    section(&fields, "WHEN", &when);

    let conditions = gtk::Box::new(gtk::Orientation::Vertical, 6);
    let condition = Choice::new(&[
        ("none", "No condition"),
        (
            "check",
            if owner == Owner::Desktop {
                "Application is running"
            } else {
                "Entity has state"
            },
        ),
    ]);
    conditions.append(&condition.widget);
    let condition_value = entry(if owner == Owner::Desktop {
        "Window class"
    } else {
        "Required state, e.g. home"
    });
    let condition_entity = if owner == Owner::HomeAssistant {
        let picker = EntityPicker::new(&home, |_| true);
        conditions.append(&picker.widget);
        Some(picker)
    } else {
        None
    };
    conditions.append(&condition_value);
    section(&fields, "IF", &conditions);

    let then = gtk::Box::new(gtk::Orientation::Vertical, 7);
    let action_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    then.append(&action_box);
    let add = gtk::Button::with_label("Add action");
    add.set_halign(gtk::Align::Start);
    then.append(&add);
    section(&fields, "THEN", &then);
    let rows = Rc::new(RefCell::new(Vec::<Rc<ActionRow>>::new()));
    let append_row: Rc<dyn Fn(Option<NodeKind>)> = {
        let rows = rows.clone();
        let action_box = action_box.clone();
        let home = home.clone();
        Rc::new(move |initial| {
            let item = row(owner, &home, initial.as_ref());
            let remove = gtk::Button::from_icon_name("list-remove-symbolic");
            remove.set_tooltip_text(Some("Remove action"));
            item.container.append(&remove);
            let weak = Rc::downgrade(&item);
            let rows_remove = rows.clone();
            let action_box_remove = action_box.clone();
            remove.connect_clicked(move |_| {
                if let Some(item) = weak.upgrade() {
                    action_box_remove.remove(&item.container);
                    rows_remove.borrow_mut().retain(|r| !Rc::ptr_eq(r, &item));
                }
            });
            action_box.append(&item.container);
            rows.borrow_mut().push(item);
        })
    };
    let add_row = append_row.clone();
    add.connect_clicked(move |_| add_row(None));
    if let Some(workflow) = &original {
        let order = workflow.validate().unwrap();
        for id in order {
            let kind = &workflow.nodes.iter().find(|n| n.id == id).unwrap().kind;
            match kind {
                NodeKind::Manual => {
                    trigger.set_active_id("manual");
                }
                NodeKind::Startup => {
                    trigger.set_active_id("startup");
                }
                NodeKind::Interval { seconds } => {
                    trigger.set_active_id("interval");
                    trigger_value.set_text(&(seconds / 60).to_string());
                }
                NodeKind::WindowOpened { class } => {
                    trigger.set_active_id("opened");
                    trigger_value.set_text(class);
                }
                NodeKind::WindowClosed { class } => {
                    trigger.set_active_id("closed");
                    trigger_value.set_text(class);
                }
                NodeKind::EntityState { entity_id, state } => {
                    trigger.set_active_id("entity");
                    trigger_value.set_text(state);
                    if let Some(p) = &trigger_entity {
                        p.set(entity_id);
                    }
                }
                NodeKind::AppRunning { class } => {
                    condition.set_active_id("check");
                    condition_value.set_text(class);
                }
                NodeKind::EntityIs { entity_id, state } => {
                    condition.set_active_id("check");
                    condition_value.set_text(state);
                    if let Some(p) = &condition_entity {
                        p.set(entity_id);
                    }
                }
                _ => append_row(Some(kind.clone())),
            }
        }
    } else {
        append_row(None);
    }
    if owner == Owner::Desktop {
        let value = trigger_value.clone();
        let ids = trigger.ids.clone();
        value.set_visible(matches!(
            trigger.active_id(),
            Some("interval" | "opened" | "closed")
        ));
        trigger.widget.connect_selected_notify(move |dropdown| {
            let selected = ids.get(dropdown.selected() as usize).map(String::as_str);
            value.set_visible(matches!(selected, Some("interval" | "opened" | "closed")));
            value.set_placeholder_text(Some(if selected == Some("interval") {
                "Minutes (1–1440)"
            } else {
                "Window class, e.g. firefox"
            }));
        });
    }
    let condition_value_copy = condition_value.clone();
    let condition_picker = condition_entity
        .as_ref()
        .map(|picker| picker.widget.clone());
    condition_value.set_visible(condition.active_id() == Some("check"));
    if let Some(picker) = &condition_picker {
        picker.set_visible(condition.active_id() == Some("check"));
    }
    let ids = condition.ids.clone();
    condition.widget.connect_selected_notify(move |dropdown| {
        let visible = ids
            .get(dropdown.selected() as usize)
            .is_some_and(|id| id == "check");
        condition_value_copy.set_visible(visible);
        if let Some(picker) = &condition_picker {
            picker.set_visible(visible);
        }
    });
    let feedback = label("");
    feedback.set_wrap(true);
    content.append(&feedback);
    let make_workflow: Rc<dyn Fn() -> Result<Workflow, String>> = {
        let original = original.clone();
        let rows = rows.clone();
        let home = home.clone();
        Rc::new(move || {
            let trigger_kind = if owner == Owner::Desktop {
                match trigger.active_id() {
                    Some("manual") => NodeKind::Manual,
                    Some("startup") => NodeKind::Startup,
                    Some("interval") => {
                        let minutes: u64 = trigger_value
                            .text()
                            .trim()
                            .parse()
                            .map_err(|_| "Enter an interval in minutes".to_string())?;
                        NodeKind::Interval {
                            seconds: minutes.saturating_mul(60),
                        }
                    }
                    Some("opened") => NodeKind::WindowOpened {
                        class: trigger_value.text().trim().into(),
                    },
                    Some("closed") => NodeKind::WindowClosed {
                        class: trigger_value.text().trim().into(),
                    },
                    _ => return Err("Choose a trigger".into()),
                }
            } else {
                NodeKind::EntityState {
                    entity_id: trigger_entity
                        .as_ref()
                        .map(EntityPicker::value)
                        .unwrap_or_default(),
                    state: trigger_value.text().trim().into(),
                }
            };
            let mut kinds = vec![trigger_kind];
            if condition.active_id() == Some("check") {
                kinds.push(if owner == Owner::Desktop {
                    NodeKind::AppRunning {
                        class: condition_value.text().trim().into(),
                    }
                } else {
                    NodeKind::EntityIs {
                        entity_id: condition_entity
                            .as_ref()
                            .map(EntityPicker::value)
                            .unwrap_or_default(),
                        state: condition_value.text().trim().into(),
                    }
                });
            }
            for item in rows.borrow().iter() {
                let value = item.value.text().trim().to_string();
                let entity = item
                    .entity
                    .as_ref()
                    .map(EntityPicker::value)
                    .unwrap_or_default();
                let kind = match (owner, item.kind.active_id()) {
                    (Owner::Desktop, Some("notify")) => NodeKind::Notify {
                        title: value,
                        body: item.body.text().to_string(),
                    },
                    (Owner::Desktop, Some("workspace")) => NodeKind::SwitchWorkspace {
                        number: value
                            .parse()
                            .map_err(|_| "Enter a workspace number".to_string())?,
                    },
                    (Owner::Desktop, Some("volume")) => NodeKind::SetVolume {
                        percent: value
                            .parse()
                            .map_err(|_| "Enter a volume percentage".to_string())?,
                    },
                    (Owner::Desktop, Some("launch")) => NodeKind::LaunchApplication {
                        desktop_id: item
                            .app
                            .as_ref()
                            .map(EntityPicker::value)
                            .unwrap_or_default(),
                    },
                    (Owner::HomeAssistant, Some("on")) => NodeKind::SetEntity {
                        entity_id: entity,
                        turn_on: true,
                    },
                    (Owner::HomeAssistant, Some("off")) => NodeKind::SetEntity {
                        entity_id: entity,
                        turn_on: false,
                    },
                    (Owner::HomeAssistant, Some("scene")) => {
                        NodeKind::ActivateScene { entity_id: entity }
                    }
                    _ => return Err("Choose an action".into()),
                };
                kinds.push(kind);
            }
            let prior = original
                .as_ref()
                .and_then(|w| w.validate().ok().map(|order| (w, order)));
            let nodes: Vec<Node> = kinds
                .into_iter()
                .enumerate()
                .map(|(i, kind)| {
                    let old = prior.as_ref().and_then(|(w, order)| {
                        order
                            .get(i)
                            .and_then(|id| w.nodes.iter().find(|node| &node.id == id))
                    });
                    let id = old
                        .filter(|node| {
                            std::mem::discriminant(&node.kind) == std::mem::discriminant(&kind)
                        })
                        .map(|node| node.id.clone())
                        .unwrap_or_else(|| format!("node-{}", uuid::Uuid::new_v4().simple()));
                    Node { id, kind }
                })
                .collect();
            let edges = (1..nodes.len())
                .map(|i| Edge {
                    from: nodes[i - 1].id.clone(),
                    to: nodes[i].id.clone(),
                    output: None,
                })
                .collect();
            let workflow = Workflow {
                schema_version: 1,
                id: original
                    .as_ref()
                    .map(|w| w.id.clone())
                    .unwrap_or_else(|| format!("home-{}", uuid::Uuid::new_v4().simple())),
                name: name.text().trim().into(),
                enabled: original.as_ref().map(|w| w.enabled).unwrap_or(true),
                owner,
                nodes,
                edges,
                positions: HashMap::new(),
            };
            workflow.validate()?;
            if owner == Owner::HomeAssistant && (home.base.is_empty() || home.token.is_empty()) {
                return Err("Connect Home Assistant first".into());
            }
            Ok(workflow)
        })
    };
    let dry_button = gtk::Button::with_label("Dry run");
    content.append(&dry_button);
    let make = make_workflow.clone();
    let report = feedback.clone();
    let home_for_dry = home.clone();
    dry_button.connect_clicked(move |_| match make() {
        Ok(workflow) => {
            let context = DryRunContext {
                entity_states: home_for_dry
                    .entities
                    .iter()
                    .map(|(id, _, state)| (id.clone(), state.clone()))
                    .collect::<HashMap<_, _>>(),
                running_apps: None,
            };
            match workflow.dry_run(&context) {
                Ok(plan) => report.set_text(&format!(
                    "{}{}",
                    plan.steps
                        .iter()
                        .map(|s| format!("{}: {}", s.node_id, s.result))
                        .collect::<Vec<_>>()
                        .join(" · "),
                    if plan.warnings.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", plan.warnings.join("; "))
                    }
                )),
                Err(e) => report.set_text(&e),
            }
        }
        Err(e) => report.set_text(&e),
    });
    let make = make_workflow.clone();
    let dialog_close = dialog.clone();
    let home_save = home.clone();
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save automation");
    save.add_css_class("suggested-action");
    footer.append(&cancel);
    footer.append(&save);
    content.append(&footer);
    let cancel_window = dialog.clone();
    cancel.connect_clicked(move |_| cancel_window.close());
    let save_button = save.clone();
    save.connect_clicked(move |_| {
        let workflow = match make() {
            Ok(w) => w,
            Err(e) => {
                feedback.set_text(&e);
                return;
            }
        };
        save_button.set_sensitive(false);
        feedback.set_text("Saving automation…");
        let (tx, rx) = async_channel::bounded(1);
        let home = home_save.clone();
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
        let dialog_done = dialog_close.clone();
        let feedback_done = feedback.clone();
        let save_done = save_button.clone();
        let saved = on_saved.clone();
        gtk::glib::MainContext::default().spawn_local(async move {
            match rx.recv().await {
                Ok(Ok(())) => {
                    dialog_done.close();
                    saved();
                }
                Ok(Err(e)) => {
                    feedback_done.set_text(&e);
                    save_done.set_sensitive(true);
                }
                Err(_) => {
                    feedback_done.set_text("Save failed");
                    save_done.set_sensitive(true);
                }
            }
        });
    });
    dialog.present();
}
