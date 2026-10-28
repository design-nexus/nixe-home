//! The simple automation builder: one trigger, an optional condition and a
//! list of actions, for desktop and Home Assistant automations.
use crate::{present, rt, service};
use gtk::prelude::*;
use nyx_home::automation::{DryRunContext, Edge, Node, NodeKind, Owner, Workflow};
use nyx_ui::widgets;
use std::{cell::RefCell, collections::HashMap, rc::Rc};

#[derive(Clone, Default)]
pub struct HomeContext {
    pub base: String,
    pub token: String,
    pub entities: Vec<(String, String, String)>, // ID, friendly name, state
}

impl HomeContext {
    pub fn name_of(&self, id: &str) -> String {
        self.entities
            .iter()
            .find(|(entity, _, _)| entity == id)
            .map_or_else(|| id.to_string(), |(_, name, _)| name.clone())
    }
}

/// A searchable dropdown over (ID, label) options.
#[derive(Clone)]
pub struct Picker {
    pub widget: gtk::DropDown,
    ids: Vec<String>,
}

impl Picker {
    pub fn entities(home: &HomeContext, filter: impl Fn(&str) -> bool) -> Self {
        let options = home
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
        options.sort_by_key(|option| option.1.to_lowercase());
        let ids: Vec<String> = options.iter().map(|(id, _)| id.clone()).collect();
        let labels: Vec<&str> = options.iter().map(|(_, name)| name.as_str()).collect();
        let widget = gtk::DropDown::from_strings(&labels);
        widget.set_expression(Some(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            None::<gtk::Expression>,
            "string",
        )));
        widget.set_enable_search(true);
        widget.set_search_match_mode(gtk::StringFilterMatchMode::Substring);
        widget.set_hexpand(true);
        widget.set_selected(gtk::INVALID_LIST_POSITION);
        Self { widget, ids }
    }
    pub fn value(&self) -> String {
        self.ids
            .get(self.widget.selected() as usize)
            .cloned()
            .unwrap_or_default()
    }
    pub fn set(&self, id: &str) {
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
    fn new(options: &[(&str, &str)], label: &str) -> Self {
        let ids = options.iter().map(|(id, _)| (*id).to_string()).collect();
        let names: Vec<_> = options.iter().map(|(_, name)| *name).collect();
        let widget = gtk::DropDown::from_strings(&names);
        widget.update_property(&[gtk::accessible::Property::Label(label)]);
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

fn entry(placeholder: &str) -> gtk::Entry {
    let entry = gtk::Entry::builder()
        .placeholder_text(placeholder)
        .hexpand(true)
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(placeholder)]);
    entry
}

fn spin(min: f64, max: f64, label: &str) -> gtk::SpinButton {
    let spin = gtk::SpinButton::with_range(min, max, 1.0);
    spin.set_numeric(true);
    spin.update_property(&[gtk::accessible::Property::Label(label)]);
    spin
}

struct ActionRow {
    container: gtk::Box,
    kind: Choice,
    title: gtk::Entry,
    body: gtk::Entry,
    number: gtk::SpinButton,
    entity: Option<Picker>,
    app: Option<Picker>,
}

impl ActionRow {
    fn sync(&self) {
        let kind = self.kind.active_id().unwrap_or("");
        self.title.set_visible(kind == "notify");
        self.body.set_visible(kind == "notify");
        self.number
            .set_visible(matches!(kind, "workspace" | "volume"));
        if let Some(app) = &self.app {
            app.widget.set_visible(kind == "launch");
        }
        let (min, max, label) = match kind {
            "workspace" => (1.0, 20.0, "Workspace number"),
            _ => (0.0, 100.0, "Volume percent"),
        };
        self.number.set_range(min, max);
        self.number
            .update_property(&[gtk::accessible::Property::Label(label)]);
        self.number.set_tooltip_text(Some(label));
    }
}

fn action_row(owner: Owner, home: &HomeContext, initial: Option<&NodeKind>) -> Rc<ActionRow> {
    let container = widgets::hbox(8);
    let kind = if owner == Owner::Desktop {
        Choice::new(
            &[
                ("notify", "Send a notification"),
                ("workspace", "Switch workspace"),
                ("volume", "Set the volume"),
                ("launch", "Open an app"),
            ],
            "Action",
        )
    } else {
        Choice::new(
            &[
                ("on", "Turn on"),
                ("off", "Turn off"),
                ("scene", "Activate a scene"),
            ],
            "Action",
        )
    };
    let title = entry("Notification title");
    let body = entry("Message");
    let number = spin(0.0, 100.0, "Value");
    number.set_value(50.0);
    let entity = (owner == Owner::HomeAssistant).then(|| {
        let picker = Picker::entities(home, |id| {
            ["light.", "switch.", "fan.", "input_boolean.", "scene."]
                .iter()
                .any(|prefix| id.starts_with(prefix))
        });
        picker
            .widget
            .update_property(&[gtk::accessible::Property::Label("Device or scene")]);
        picker
    });
    let app = (owner == Owner::Desktop).then(|| {
        let picker = Picker::applications();
        picker
            .widget
            .update_property(&[gtk::accessible::Property::Label("App to open")]);
        picker
    });
    container.append(&kind.widget);
    if let Some(picker) = &entity {
        container.append(&picker.widget);
    } else {
        container.append(&title);
        container.append(&body);
        container.append(&number);
        if let Some(picker) = &app {
            container.append(&picker.widget);
        }
    }
    let row = Rc::new(ActionRow {
        container,
        kind,
        title,
        body,
        number,
        entity,
        app,
    });
    if let Some(initial) = initial {
        match initial {
            NodeKind::Notify { title, body } => {
                row.kind.set_active_id("notify");
                row.title.set_text(title);
                row.body.set_text(body);
            }
            NodeKind::SwitchWorkspace { number } => {
                row.kind.set_active_id("workspace");
                row.sync();
                row.number.set_value(*number as f64);
            }
            NodeKind::SetVolume { percent } => {
                row.kind.set_active_id("volume");
                row.sync();
                row.number.set_value(*percent as f64);
            }
            NodeKind::LaunchApplication { desktop_id } => {
                row.kind.set_active_id("launch");
                if let Some(app) = &row.app {
                    app.set(desktop_id);
                }
            }
            NodeKind::SetEntity { entity_id, turn_on } => {
                row.kind.set_active_id(if *turn_on { "on" } else { "off" });
                if let Some(picker) = &row.entity {
                    picker.set(entity_id);
                }
            }
            NodeKind::ActivateScene { entity_id } => {
                row.kind.set_active_id("scene");
                if let Some(picker) = &row.entity {
                    picker.set(entity_id);
                }
            }
            _ => {}
        }
    }
    if owner == Owner::Desktop {
        let weak = Rc::downgrade(&row);
        row.kind.widget.connect_selected_notify(move |_| {
            if let Some(row) = weak.upgrade() {
                row.sync();
            }
        });
        row.sync();
    }
    row
}

fn section(content: &gtk::Box, title: &str, hint: &str, widget: &impl IsA<gtk::Widget>) {
    let card = widgets::vbox(8);
    card.add_css_class("card");
    card.add_css_class("choice-card");
    card.append(&widgets::label(title, "card-title"));
    if !hint.is_empty() {
        let note = widgets::label(hint, "dim");
        note.set_wrap(true);
        card.append(&note);
    }
    card.append(widget);
    content.append(&card);
}

/// Feedback lines and the style class for each.
type Lines = Vec<(String, &'static str)>;

fn existing_simple(workflow: &Workflow) -> bool {
    workflow.owner != Owner::HybridDesktop
        && workflow.policies.is_empty()
        && workflow.groups.is_empty()
        && workflow.edges.iter().all(|e| e.output.is_none())
        && workflow.validate().is_ok()
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

/// Explain that a graph needs the canvas, and offer to open it there.
fn needs_canvas(workflow: Workflow) {
    let (dialog, card) = widgets::dialog(&workflow.name, 420);
    let message = widgets::label(
        "This automation has branches, parallel paths, waits or retries, which the simple builder can't show. Edit it on the canvas instead; nothing has been changed.",
        "dim",
    );
    message.set_wrap(true);
    card.append(&message);
    let buttons = widgets::hbox(8);
    buttons.set_halign(gtk::Align::End);
    let close = gtk::Button::with_label("Close");
    let open = gtk::Button::with_label("Open in canvas");
    open.add_css_class("suggested-action");
    buttons.append(&close);
    buttons.append(&open);
    card.append(&buttons);
    let d = dialog.clone();
    close.connect_clicked(move |_| d.close());
    let d = dialog.clone();
    open.connect_clicked(move |_| {
        d.close();
        crate::automations::open_canvas(workflow.clone());
    });
    dialog.present();
}

pub fn present(
    _parent: &gtk::Window,
    original: Option<Workflow>,
    owner: Owner,
    home: HomeContext,
    on_saved: Rc<dyn Fn()>,
) {
    if let Some(workflow) = &original
        && !existing_simple(workflow)
    {
        needs_canvas(workflow.clone());
        return;
    }
    let (dialog, card) = widgets::dialog(
        if original.is_some() {
            "Edit automation"
        } else if owner == Owner::HomeAssistant {
            "New Home Assistant automation"
        } else {
            "New desktop automation"
        },
        660,
    );
    dialog.set_default_height(640);
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::External)
        .propagate_natural_height(true)
        .max_content_height(520)
        .build();
    let fields = widgets::vbox(12);
    scroll.set_child(Some(&fields));
    card.append(&scroll);

    let name = entry("Name, e.g. Focus mode");
    let first = name.clone();
    if let Some(w) = &original {
        name.set_text(&w.name);
    }
    section(&fields, "Name", "", &name);

    let when = widgets::vbox(8);
    let trigger = if owner == Owner::Desktop {
        Choice::new(
            &[
                ("manual", "Only when I run it"),
                ("startup", "When the desktop starts"),
                ("interval", "On a schedule"),
                ("opened", "When a window opens"),
                ("closed", "When a window closes"),
            ],
            "Trigger",
        )
    } else {
        Choice::new(&[("entity", "When an entity changes state")], "Trigger")
    };
    when.append(&trigger.widget);
    let minutes = spin(1.0, 1440.0, "Every how many minutes");
    minutes.set_value(15.0);
    let minutes_row = widgets::hbox(8);
    minutes_row.append(&widgets::label("Every", ""));
    minutes_row.append(&minutes);
    minutes_row.append(&widgets::label("minutes", ""));
    let window_class = entry("Window class, e.g. firefox");
    let trigger_entity = (owner == Owner::HomeAssistant).then(|| {
        let picker = Picker::entities(&home, |_| true);
        picker
            .widget
            .update_property(&[gtk::accessible::Property::Label("Entity that changes")]);
        when.append(&picker.widget);
        picker
    });
    let trigger_state = entry("New state, e.g. on");
    let class_hint = widgets::label(
        "The class is what Hyprland reports for the window; run hyprctl clients to see them.",
        "dim",
    );
    class_hint.set_wrap(true);
    if owner == Owner::Desktop {
        when.append(&minutes_row);
        when.append(&window_class);
        when.append(&class_hint);
    } else {
        when.append(&trigger_state);
    }
    section(&fields, "When", "", &when);

    let conditions = widgets::vbox(8);
    let condition = Choice::new(
        &[
            ("none", "Always"),
            (
                "check",
                if owner == Owner::Desktop {
                    "Only if an app is running"
                } else {
                    "Only if an entity has a state"
                },
            ),
        ],
        "Condition",
    );
    conditions.append(&condition.widget);
    let condition_value = entry(if owner == Owner::Desktop {
        "Window class of the app"
    } else {
        "Required state, e.g. home"
    });
    let condition_entity = (owner == Owner::HomeAssistant).then(|| {
        let picker = Picker::entities(&home, |_| true);
        picker
            .widget
            .update_property(&[gtk::accessible::Property::Label("Entity to check")]);
        conditions.append(&picker.widget);
        picker
    });
    conditions.append(&condition_value);
    section(&fields, "Only if", "", &conditions);

    let then = widgets::vbox(8);
    let action_box = widgets::vbox(8);
    then.append(&action_box);
    let add = widgets::labeled_button("list-add-symbolic", "Add action");
    add.add_css_class("flat");
    add.set_halign(gtk::Align::Start);
    then.append(&add);
    section(&fields, "Then", "Actions run in order.", &then);

    let rows = Rc::new(RefCell::new(Vec::<Rc<ActionRow>>::new()));
    let append_row: Rc<dyn Fn(Option<NodeKind>)> = {
        let rows = rows.clone();
        let action_box = action_box.clone();
        let home = home.clone();
        Rc::new(move |initial| {
            let item = action_row(owner, &home, initial.as_ref());
            let remove = widgets::icon_button("list-remove-symbolic", "Remove this action");
            remove.update_property(&[gtk::accessible::Property::Label("Remove this action")]);
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

    if let Some(workflow) = &original
        && let Ok(order) = workflow.validate()
    {
        for id in order {
            let Some(kind) = workflow.nodes.iter().find(|n| n.id == id).map(|n| &n.kind) else {
                continue;
            };
            match kind {
                NodeKind::Manual => trigger.set_active_id("manual"),
                NodeKind::Startup => trigger.set_active_id("startup"),
                NodeKind::Interval { seconds } => {
                    trigger.set_active_id("interval");
                    minutes.set_value((*seconds / 60) as f64);
                }
                NodeKind::WindowOpened { class } => {
                    trigger.set_active_id("opened");
                    window_class.set_text(class);
                }
                NodeKind::WindowClosed { class } => {
                    trigger.set_active_id("closed");
                    window_class.set_text(class);
                }
                NodeKind::EntityState { entity_id, state } => {
                    trigger.set_active_id("entity");
                    trigger_state.set_text(state);
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
        let sync = {
            let (trigger, minutes_row, window_class, class_hint) = (
                trigger.clone(),
                minutes_row.clone(),
                window_class.clone(),
                class_hint.clone(),
            );
            move || {
                let selected = trigger.active_id();
                minutes_row.set_visible(selected == Some("interval"));
                let window = matches!(selected, Some("opened" | "closed"));
                window_class.set_visible(window);
                class_hint.set_visible(window);
            }
        };
        sync();
        trigger.widget.connect_selected_notify(move |_| sync());
    }
    {
        let sync = {
            let (condition, value, picker) = (
                condition.clone(),
                condition_value.clone(),
                condition_entity.as_ref().map(|p| p.widget.clone()),
            );
            move || {
                let visible = condition.active_id() == Some("check");
                value.set_visible(visible);
                if let Some(picker) = &picker {
                    picker.set_visible(visible);
                }
            }
        };
        sync();
        condition.widget.connect_selected_notify(move |_| sync());
    }

    let feedback = widgets::vbox(4);
    feedback.add_css_class("builder-feedback");
    feedback.set_visible(false);
    card.append(&feedback);
    let say = {
        let feedback = feedback.clone();
        move |lines: Lines| {
            while let Some(child) = feedback.first_child() {
                feedback.remove(&child);
            }
            for (text, class) in lines {
                let label = widgets::label(&text, class);
                label.set_wrap(true);
                feedback.append(&label);
            }
            feedback.set_visible(feedback.first_child().is_some());
        }
    };
    let say: Rc<dyn Fn(Lines)> = Rc::new(say);

    let make_workflow: Rc<dyn Fn() -> Result<Workflow, String>> = {
        let original = original.clone();
        let rows = rows.clone();
        let home = home.clone();
        Rc::new(move || {
            let trigger_kind = if owner == Owner::Desktop {
                match trigger.active_id() {
                    Some("manual") => NodeKind::Manual,
                    Some("startup") => NodeKind::Startup,
                    Some("interval") => NodeKind::Interval {
                        seconds: (minutes.value() as u64).saturating_mul(60),
                    },
                    Some("opened") => NodeKind::WindowOpened {
                        class: window_class.text().trim().into(),
                    },
                    Some("closed") => NodeKind::WindowClosed {
                        class: window_class.text().trim().into(),
                    },
                    _ => return Err("Choose when it runs.".into()),
                }
            } else {
                NodeKind::EntityState {
                    entity_id: trigger_entity
                        .as_ref()
                        .map(Picker::value)
                        .unwrap_or_default(),
                    state: trigger_state.text().trim().into(),
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
                            .map(Picker::value)
                            .unwrap_or_default(),
                        state: condition_value.text().trim().into(),
                    }
                });
            }
            for item in rows.borrow().iter() {
                let entity = item.entity.as_ref().map(Picker::value).unwrap_or_default();
                let kind = match (owner, item.kind.active_id()) {
                    (Owner::Desktop, Some("notify")) => NodeKind::Notify {
                        title: item.title.text().trim().into(),
                        body: item.body.text().to_string(),
                    },
                    (Owner::Desktop, Some("workspace")) => NodeKind::SwitchWorkspace {
                        number: item.number.value() as u8,
                    },
                    (Owner::Desktop, Some("volume")) => NodeKind::SetVolume {
                        percent: item.number.value() as u8,
                    },
                    (Owner::Desktop, Some("launch")) => NodeKind::LaunchApplication {
                        desktop_id: item.app.as_ref().map(Picker::value).unwrap_or_default(),
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
                    _ => return Err("Choose an action.".into()),
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
                policies: HashMap::new(),
                groups: Vec::new(),
            };
            if workflow.name.is_empty() {
                return Err("Give the automation a name.".into());
            }
            workflow
                .validate()
                .map_err(|e| present::explain(&e, &workflow, &|id| home.name_of(id)))?;
            if owner == Owner::HomeAssistant && (home.base.is_empty() || home.token.is_empty()) {
                return Err("Connect Home Assistant first.".into());
            }
            Ok(workflow)
        })
    };

    let footer = widgets::hbox(8);
    footer.set_margin_top(6);
    let dry_button = widgets::labeled_button("media-playback-start-symbolic", "Dry run");
    dry_button.set_tooltip_text(Some("See what would happen now, without doing anything"));
    footer.append(&dry_button);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    footer.append(&cancel);
    footer.append(&save);
    card.append(&footer);

    {
        let (make, say, home) = (make_workflow.clone(), say.clone(), home.clone());
        dry_button.connect_clicked(move |_| match make() {
            Ok(workflow) => {
                let context = DryRunContext {
                    entity_states: home
                        .entities
                        .iter()
                        .map(|(id, _, state)| (id.clone(), state.clone()))
                        .collect(),
                    running_apps: None,
                };
                match workflow.dry_run(&context) {
                    Ok(plan) => {
                        let names = |id: &str| home.name_of(id);
                        let mut lines = vec![("If it ran now:".to_string(), "card-title")];
                        for step in plan.steps {
                            let label = workflow
                                .nodes
                                .iter()
                                .find(|n| n.id == step.node_id)
                                .map(|n| present::node_label(&n.kind, &names))
                                .unwrap_or(step.node_id);
                            let skipped =
                                step.result.contains("skip") || step.result.contains("false");
                            lines.push((
                                format!("{label}: {}", step.result),
                                if skipped { "dim" } else { "" },
                            ));
                        }
                        for warning in plan.warnings {
                            lines.push((warning, "warning-text"));
                        }
                        say(lines);
                    }
                    Err(e) => say(vec![(e, "error-text")]),
                }
            }
            Err(e) => say(vec![(e, "error-text")]),
        });
    }
    {
        let d = dialog.clone();
        cancel.connect_clicked(move |_| d.close());
    }
    let save_button = save.clone();
    let shown = dialog.clone();
    save.connect_clicked(move |_| {
        let workflow = match make_workflow() {
            Ok(w) => w,
            Err(e) => {
                say(vec![(e, "error-text")]);
                return;
            }
        };
        save_button.set_sensitive(false);
        save_button.set_label("Saving…");
        let home = home.clone();
        let (dialog, say, save_button, on_saved) = (
            dialog.clone(),
            say.clone(),
            save_button.clone(),
            on_saved.clone(),
        );
        let name = workflow.name.clone();
        rt::spawn(
            async move {
                if workflow.owner == Owner::HomeAssistant {
                    crate::ha_automation::save(&home.base, &home.token, &workflow).await?;
                }
                service::save(&workflow).await
            },
            move |result| match result {
                Ok(()) => {
                    dialog.close();
                    on_saved();
                    crate::app::toast(&format!("Saved {name}."));
                }
                Err(e) => {
                    say(vec![(e, "error-text")]);
                    save_button.set_sensitive(true);
                    save_button.set_label("Save");
                }
            },
        );
    });
    shown.present();
    first.grab_focus();
}
