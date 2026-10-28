//! Automations: the list of desktop, Home Assistant and hybrid automations,
//! and where new ones start.
use crate::{
    app::{self, Home, Section},
    builder::{self, HomeContext},
    canvas, ha_automation, present, rt, service,
};
use gtk::prelude::*;
use nyx_home::automation::{Edge, Node, NodeKind, Owner, Workflow};
use nyx_ui::widgets;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
};

pub struct Page {
    pub root: gtk::Widget,
    pub actions: gtk::Box,
    stack: gtk::Stack,
    problem: gtk::Box,
    list: gtk::ListBox,
    count: gtk::Label,
    workflows: RefCell<Vec<Workflow>>,
    /// Home Assistant's own on/off state for the automations it runs, by config ID.
    ha_states: RefCell<HashMap<String, String>>,
    loaded: Cell<bool>,
}

impl Page {
    pub fn new() -> Self {
        let stack = gtk::Stack::new();
        let problem = widgets::vbox(0);
        problem.set_vexpand(true);
        stack.add_named(&problem, Some("problem"));
        let body = widgets::vbox(0);
        body.add_css_class("settings-page");
        body.add_css_class("home-page");
        let head = widgets::hbox(8);
        let count = widgets::label("", "group-title");
        count.set_hexpand(true);
        head.append(&count);
        body.append(&head);
        let list = gtk::ListBox::new();
        list.add_css_class("group-list");
        list.add_css_class("automation-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.update_property(&[gtk::accessible::Property::Label("Automations")]);
        list.connect_row_activated(|_, row| {
            let id = row.widget_name().to_string();
            if let Some(app) = app::get() {
                let workflow = app
                    .automations
                    .workflows
                    .borrow()
                    .iter()
                    .find(|w| w.id == id)
                    .cloned();
                if let Some(workflow) = workflow {
                    edit(workflow);
                }
            }
        });
        body.append(&list);
        let note = widgets::label(
            "Desktop and hybrid automations run in the Home service on this computer, even with this window closed. Home Assistant automations run in Home Assistant.",
            "group-note",
        );
        note.set_wrap(true);
        note.set_margin_top(12);
        body.append(&note);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .child(&body)
            .build();
        stack.add_named(&scroll, Some("list"));
        problem.append(&widgets::empty_state(
            "system-run-symbolic",
            "Loading automations…",
            "",
            None,
        ));

        let actions = widgets::hbox(2);
        let new = widgets::bar_button("list-add-symbolic", "New automation (Ctrl+N)");
        new.add_css_class("suggested-action");
        new.update_property(&[gtk::accessible::Property::Label("New automation")]);
        new.connect_clicked(|_| new_automation());
        actions.append(&new);
        Self {
            root: stack.clone().upcast(),
            actions,
            stack,
            problem,
            list,
            count,
            workflows: RefCell::new(Vec::new()),
            ha_states: RefCell::new(HashMap::new()),
            loaded: Cell::new(false),
        }
    }

    pub fn reload(&self) {
        rt::spawn(service::workflows(), |result| {
            let Some(app) = app::get() else { return };
            let page = &app.automations;
            match result {
                Ok(workflows) => {
                    page.loaded.set(true);
                    *page.workflows.borrow_mut() = workflows;
                    page.render(&app.home.borrow());
                }
                Err(error) => page.show_problem(&error),
            }
        });
    }

    /// Home Assistant's automation states changed (or it connected).
    pub fn home_changed(&self, home: &Home) {
        let states = automation_states(home);
        if *self.ha_states.borrow() != states {
            *self.ha_states.borrow_mut() = states;
            if self.loaded.get() {
                self.render(home);
            }
        }
    }

    fn show_problem(&self, error: &str) {
        while let Some(child) = self.problem.first_child() {
            self.problem.remove(&child);
        }
        self.problem.append(&widgets::empty_state(
            "dialog-warning-symbolic",
            "The Home service isn't available",
            &format!(
                "{} Desktop automations need it. Start it with <tt>systemctl --user start nyx-homed</tt>.",
                gtk::glib::markup_escape_text(error)
            ),
            Some(("Try again", Box::new(|| {
                if let Some(app) = app::get() {
                    app.automations.reload();
                }
            }))),
        ));
        self.stack.set_visible_child_name("problem");
    }

    fn render(&self, home: &Home) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let workflows = self.workflows.borrow();
        if workflows.is_empty() {
            while let Some(child) = self.problem.first_child() {
                self.problem.remove(&child);
            }
            self.problem.append(&widgets::empty_state(
                crate::icons::AUTOMATIONS,
                "No automations yet",
                "Automations turn things on, send notifications or switch workspaces when something happens: a window opens, a sensor changes, or it's time.",
                Some(("New automation…", Box::new(new_automation))),
            ));
            self.stack.set_visible_child_name("problem");
            return;
        }
        self.count
            .set_text(&present::plural(workflows.len(), "automation").to_uppercase());
        let names = |id: &str| home.name_of(id);
        for workflow in workflows.iter() {
            self.list.append(&self.row(workflow, &names, home));
        }
        self.stack.set_visible_child_name("list");
    }

    fn enabled(&self, workflow: &Workflow) -> bool {
        if workflow.owner == Owner::HomeAssistant {
            self.ha_states
                .borrow()
                .get(&workflow.id)
                .map_or(workflow.enabled, |state| state == "on")
        } else {
            workflow.enabled
        }
    }

    fn row(
        &self,
        workflow: &Workflow,
        names: &dyn Fn(&str) -> String,
        home: &Home,
    ) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&workflow.id);
        row.set_activatable(true);
        let content = widgets::hbox(12);
        content.add_css_class("automation-row");
        let icon = gtk::Image::from_icon_name(match workflow.owner {
            Owner::Desktop => "computer-symbolic",
            Owner::HomeAssistant => "user-home-symbolic",
            Owner::HybridDesktop => "network-server-symbolic",
        });
        icon.add_css_class("row-icon");
        icon.set_accessible_role(gtk::AccessibleRole::Presentation);
        content.append(&icon);
        let text = widgets::vbox(2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let title = widgets::label(&workflow.name, "settings-option-title");
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&title);
        let summary = widgets::label(
            &present::workflow_summary(workflow, names),
            "settings-option-description",
        );
        summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        text.append(&summary);
        content.append(&text);
        content.append(&widgets::tag(
            present::owner_label(workflow.owner),
            if workflow.owner == Owner::HomeAssistant {
                "accent"
            } else {
                ""
            },
        ));
        let enabled = self.enabled(workflow);
        let switch = gtk::Switch::new();
        switch.set_active(enabled);
        switch.set_valign(gtk::Align::Center);
        switch.set_tooltip_text(Some(if enabled {
            "On: runs when triggered"
        } else {
            "Off: doesn't run"
        }));
        switch.update_property(&[gtk::accessible::Property::Label(&format!(
            "{} enabled",
            workflow.name
        ))]);
        let needs_home = workflow.owner == Owner::HomeAssistant && home.token.is_empty();
        switch.set_sensitive(!needs_home);
        {
            let workflow = workflow.clone();
            switch.connect_state_set(move |_, on| {
                set_enabled(&workflow, on);
                gtk::glib::Propagation::Proceed
            });
        }
        content.append(&switch);
        content.append(&menu(workflow, enabled, needs_home));
        row.set_child(Some(&content));
        row.update_property(&[gtk::accessible::Property::Label(&format!(
            "{}, {}, {}",
            workflow.name,
            present::owner_label(workflow.owner),
            if enabled { "on" } else { "off" }
        ))]);
        row
    }
}

fn automation_states(home: &Home) -> HashMap<String, String> {
    home.entities
        .values()
        .filter(|e| e.domain() == "automation")
        .filter_map(|e| {
            e.attributes["id"]
                .as_str()
                .map(|id| (id.to_string(), e.state.clone()))
        })
        .collect()
}

/// What the builder and canvas need to know about Home Assistant.
pub fn context() -> HomeContext {
    let Some(app) = app::get() else {
        return HomeContext::default();
    };
    let home = app.home.borrow();
    HomeContext {
        base: home.base.clone(),
        token: home.token.clone(),
        entities: home
            .entities
            .values()
            .map(|e| (e.entity_id.clone(), e.name(), e.state.clone()))
            .collect(),
    }
}

fn menu(workflow: &Workflow, enabled: bool, needs_home: bool) -> gtk::MenuButton {
    let button = gtk::MenuButton::new();
    button.set_icon_name("view-more-symbolic");
    button.add_css_class("bar-button");
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some("More actions"));
    button.update_property(&[gtk::accessible::Property::Label(&format!(
        "More actions for {}",
        workflow.name
    ))]);
    let workflow = workflow.clone();
    button.set_create_popup_func(move |button| {
        let popover = gtk::Popover::new();
        popover.add_css_class("menu-popover");
        let items = widgets::vbox(0);
        let item = |label: &str, run: Box<dyn Fn()>| {
            let b = gtk::Button::with_label(label);
            b.add_css_class("menu-item");
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_xalign(0.0);
            }
            let p = popover.clone();
            b.connect_clicked(move |_| {
                p.popdown();
                run();
            });
            items.append(&b);
        };
        let w = workflow.clone();
        item("Edit", Box::new(move || edit(w.clone())));
        let w = workflow.clone();
        item("Open in canvas", Box::new(move || open_canvas(w.clone())));
        if !needs_home {
            let w = workflow.clone();
            item(
                if workflow.owner == Owner::HomeAssistant {
                    "Run in Home Assistant"
                } else if enabled {
                    "Run now"
                } else {
                    "Test run"
                },
                Box::new(move || run(&w)),
            );
        }
        items.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let w = workflow.clone();
        let p = popover.clone();
        let delete = widgets::two_click("Delete…", "Click again to delete", move || {
            p.popdown();
            delete(&w);
        });
        delete.add_css_class("menu-item");
        delete.set_sensitive(workflow.owner != Owner::HomeAssistant || !needs_home);
        items.append(&delete);
        popover.set_child(Some(&items));
        button.set_popover(Some(&popover));
    });
    button
}

fn reload() {
    if let Some(app) = app::get() {
        app.automations.reload();
    }
}

fn set_enabled(workflow: &Workflow, on: bool) {
    let context = context();
    let workflow = workflow.clone();
    rt::spawn(
        async move {
            if workflow.owner == Owner::HomeAssistant {
                ha_automation::control(
                    &context.base,
                    &context.token,
                    &workflow.id,
                    if on { "turn_on" } else { "turn_off" },
                )
                .await?;
            }
            service::set_enabled(&workflow.id, on).await
        },
        |result| {
            if let Err(error) = result {
                app::toast(&error);
            }
            reload();
        },
    );
}

fn run(workflow: &Workflow) {
    let context = context();
    let workflow = workflow.clone();
    let name = workflow.name.clone();
    let owner = workflow.owner;
    rt::spawn(
        async move {
            if workflow.owner == Owner::HomeAssistant {
                ha_automation::control(&context.base, &context.token, &workflow.id, "trigger")
                    .await
                    .map(|_| String::new())
            } else {
                service::run(&workflow.id).await
            }
        },
        move |result| match result {
            Ok(_) if owner == Owner::HomeAssistant => {
                app::toast(&format!("Home Assistant is running {name}."))
            }
            Ok(_) => app::toast_action(&format!("Running {name}."), "View activity", || {
                app::show(Section::Activity)
            }),
            Err(error) => app::toast(&error),
        },
    );
}

fn delete(workflow: &Workflow) {
    let context = context();
    let workflow = workflow.clone();
    let name = workflow.name.clone();
    rt::spawn(
        async move {
            if workflow.owner == Owner::HomeAssistant {
                ha_automation::delete(&context.base, &context.token, &workflow.id).await?;
            }
            service::delete(&workflow.id).await
        },
        move |result| {
            match result {
                Ok(_) => app::toast(&format!("Deleted {name}.")),
                Err(error) => app::toast(&error),
            }
            reload();
        },
    );
}

fn parent() -> Option<gtk::Window> {
    app::get().map(|app| app.window.clone().upcast())
}

pub fn edit(workflow: Workflow) {
    if workflow.owner == Owner::HybridDesktop {
        open_canvas(workflow);
        return;
    }
    let Some(parent) = parent() else { return };
    let owner = workflow.owner;
    builder::present(&parent, Some(workflow), owner, context(), Rc::new(reload));
}

pub fn open_canvas(workflow: Workflow) {
    let Some(parent) = parent() else { return };
    canvas::present(&parent, workflow, context(), Rc::new(reload));
}

/// A new desktop workflow to start the canvas with.
pub fn starter_desktop() -> Workflow {
    starter(Owner::Desktop, &HomeContext::default()).expect("a desktop starter needs no entity")
}

fn starter(owner: Owner, home: &HomeContext) -> Option<Workflow> {
    let action = match owner {
        Owner::HybridDesktop => NodeKind::SetEntity {
            entity_id: home
                .entities
                .iter()
                .map(|(id, _, _)| id)
                .find(|id| {
                    ["light.", "switch.", "fan.", "input_boolean."]
                        .iter()
                        .any(|p| id.starts_with(p))
                })?
                .clone(),
            turn_on: true,
        },
        _ => NodeKind::Notify {
            title: "Home".into(),
            body: "Automation ran".into(),
        },
    };
    Some(Workflow {
        schema_version: 1,
        id: format!("home-{}", uuid::Uuid::new_v4().simple()),
        name: match owner {
            Owner::HybridDesktop => "New hybrid automation".into(),
            _ => "New desktop automation".into(),
        },
        enabled: true,
        owner,
        nodes: vec![
            Node {
                id: "trigger".into(),
                kind: NodeKind::Manual,
            },
            Node {
                id: "action".into(),
                kind: action,
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
    })
}

/// Choose where a new automation runs, then how to build it.
pub fn new_automation() {
    let Some(parent) = parent() else { return };
    let home = context();
    let connected = !home.token.is_empty() && !home.entities.is_empty();
    let (dialog, card) = widgets::dialog("New automation", 560);
    let intro = widgets::label("Where should it run?", "dim");
    card.append(&intro);
    let options: [(Owner, &str, &str, &str); 3] = [
        (
            Owner::Desktop,
            "computer-symbolic",
            "On this computer",
            "Reacts to windows, the desktop starting or a schedule. Sends notifications, opens apps, switches workspaces and sets the volume.",
        ),
        (
            Owner::HomeAssistant,
            "user-home-symbolic",
            "In Home Assistant",
            "Runs on your Home Assistant server, even when this computer is off. When an entity changes, turn things on or off or activate a scene.",
        ),
        (
            Owner::HybridDesktop,
            "network-server-symbolic",
            "Both: hybrid",
            "Runs on this computer and reacts to or controls Home Assistant devices. Built on the canvas.",
        ),
    ];
    for (owner, icon, title, description) in options {
        let option = widgets::hbox(14);
        option.add_css_class("card");
        option.add_css_class("choice-card");
        let image = gtk::Image::from_icon_name(icon);
        image.add_css_class("row-icon");
        image.set_valign(gtk::Align::Start);
        option.append(&image);
        let text = widgets::vbox(4);
        text.set_hexpand(true);
        text.append(&widgets::label(title, "card-title"));
        let d = widgets::label(description, "dim");
        d.set_wrap(true);
        text.append(&d);
        let needs_home = owner != Owner::Desktop && !connected;
        if needs_home {
            text.append(&widgets::label(
                "Connect Home Assistant to use this.",
                "warning-text",
            ));
        }
        let buttons = widgets::hbox(8);
        buttons.set_margin_top(6);
        if owner != Owner::HybridDesktop {
            let simple = gtk::Button::with_label("Simple builder");
            simple.set_sensitive(!needs_home);
            let (dialog, parent, home) = (dialog.clone(), parent.clone(), home.clone());
            simple.connect_clicked(move |_| {
                dialog.close();
                builder::present(&parent, None, owner, home.clone(), Rc::new(reload));
            });
            buttons.append(&simple);
        }
        if owner != Owner::HomeAssistant {
            let canvas_button = gtk::Button::with_label("Canvas");
            canvas_button.set_tooltip_text(Some("Branches, parallel paths, waits and retries"));
            let workflow = starter(owner, &home);
            canvas_button.set_sensitive(!needs_home && workflow.is_some());
            let dialog = dialog.clone();
            canvas_button.connect_clicked(move |_| {
                dialog.close();
                if let Some(workflow) = workflow.clone() {
                    open_canvas(workflow);
                }
            });
            buttons.append(&canvas_button);
        }
        text.append(&buttons);
        option.append(&text);
        card.append(&option);
    }
    let cancel = gtk::Button::with_label("Cancel");
    cancel.set_halign(gtk::Align::End);
    let d = dialog.clone();
    cancel.connect_clicked(move |_| d.close());
    card.append(&cancel);
    dialog.present();
}
