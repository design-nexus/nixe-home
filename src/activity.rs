//! Activity: what desktop and hybrid automations did, with each step.
use crate::{
    app::{self, Section},
    present, rt,
    service::{self, Run},
};
use gtk::{glib, prelude::*};
use nyx_home::automation::Workflow;
use nyx_ui::widgets;
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap},
    time::Duration,
};

pub struct Page {
    pub root: gtk::Widget,
    pub actions: gtk::Box,
    stack: gtk::Stack,
    problem: gtk::Box,
    list: gtk::ListBox,
    runs: RefCell<Vec<Run>>,
    workflows: RefCell<HashMap<String, Workflow>>,
    failed_only: Cell<bool>,
    expanded: RefCell<BTreeSet<String>>,
    watching: Cell<bool>,
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
        let list = gtk::ListBox::new();
        list.add_css_class("group-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        list.update_property(&[gtk::accessible::Property::Label("Automation runs")]);
        list.connect_row_activated(|_, row| {
            if let Some(app) = app::get() {
                app.activity.toggle(row);
            }
        });
        body.append(&list);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::External)
            .child(&body)
            .build();
        stack.add_named(&scroll, Some("list"));

        let actions = widgets::hbox(6);
        let filter = widgets::segmented(
            &widgets::opts(&[("all", "All"), ("failed", "Failed")]),
            "all",
            |value| {
                if let Some(app) = app::get() {
                    app.activity.failed_only.set(value == "failed");
                    app.activity.render();
                }
            },
        );
        filter.update_property(&[gtk::accessible::Property::Label("Show runs")]);
        actions.append(&filter);
        let refresh = widgets::bar_button("view-refresh-symbolic", "Refresh (F5)");
        refresh.update_property(&[gtk::accessible::Property::Label("Refresh")]);
        refresh.connect_clicked(|_| {
            if let Some(app) = app::get() {
                app.activity.reload();
            }
        });
        actions.append(&refresh);
        Self {
            root: stack.clone().upcast(),
            actions,
            stack,
            problem,
            list,
            runs: RefCell::new(Vec::new()),
            workflows: RefCell::new(HashMap::new()),
            failed_only: Cell::new(false),
            expanded: RefCell::new(BTreeSet::new()),
            watching: Cell::new(false),
        }
    }

    pub fn reload(&self) {
        rt::spawn(
            async { (service::workflows().await, service::history(100).await) },
            |(workflows, history)| {
                let Some(app) = app::get() else { return };
                let page = &app.activity;
                match history {
                    Ok(runs) => {
                        *page.workflows.borrow_mut() = workflows
                            .unwrap_or_default()
                            .into_iter()
                            .map(|w| (w.id.clone(), w))
                            .collect();
                        let changed = *page.runs.borrow() != runs;
                        *page.runs.borrow_mut() = runs;
                        if changed || page.list.first_child().is_none() {
                            page.render();
                        }
                    }
                    Err(error) => page.show_problem(&error),
                }
            },
        );
        self.watch();
    }

    /// Refresh every few seconds while the page is on screen.
    fn watch(&self) {
        if self.watching.replace(true) {
            return;
        }
        glib::timeout_add_local(Duration::from_secs(5), || {
            let Some(app) = app::get() else {
                return glib::ControlFlow::Break;
            };
            if app::current() != Section::Activity || !app.window.is_visible() {
                app.activity.watching.set(false);
                return glib::ControlFlow::Break;
            }
            if app.window.is_active() {
                app.activity.reload();
            }
            glib::ControlFlow::Continue
        });
    }

    fn show_problem(&self, error: &str) {
        while let Some(child) = self.problem.first_child() {
            self.problem.remove(&child);
        }
        self.problem.append(&widgets::empty_state(
            "dialog-warning-symbolic",
            "The Home service isn't available",
            &format!(
                "{} Run history comes from it. Start it with <tt>systemctl --user start nyx-homed</tt>.",
                glib::markup_escape_text(error)
            ),
            Some(("Try again", Box::new(|| {
                if let Some(app) = app::get() {
                    app.activity.reload();
                }
            }))),
        ));
        self.stack.set_visible_child_name("problem");
    }

    fn name_of(&self, entity_id: &str) -> String {
        app::get().map_or_else(
            || entity_id.to_string(),
            |app| app.home.borrow().name_of(entity_id),
        )
    }

    fn render(&self) {
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        let runs = self.runs.borrow();
        let failed_only = self.failed_only.get();
        let shown: Vec<&Run> = runs
            .iter()
            .filter(|r| {
                !failed_only
                    || matches!(
                        r.status.as_str(),
                        "failed" | "interrupted" | "completed_with_errors"
                    )
            })
            .collect();
        if shown.is_empty() {
            while let Some(child) = self.problem.first_child() {
                self.problem.remove(&child);
            }
            self.problem.append(&if failed_only && !runs.is_empty() {
                widgets::empty_state("emblem-ok-symbolic", "No failed runs", "Every recent run finished without errors.", None)
            } else {
                widgets::empty_state(
                    "document-open-recent-symbolic",
                    "No runs yet",
                    "When a desktop or hybrid automation runs, you'll see what it did here. Home Assistant keeps its own traces.",
                    None,
                )
            });
            self.stack.set_visible_child_name("problem");
            return;
        }
        let now = glib::DateTime::now_utc().map_or(0, |d| d.to_unix());
        let names = |id: &str| self.name_of(id);
        let workflows = self.workflows.borrow();
        for run in shown {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&run.id);
            let column = widgets::vbox(6);
            column.add_css_class("run-row");
            let line = widgets::hbox(12);
            let (status, icon, tone) = present::status(&run.status);
            let image = gtk::Image::from_icon_name(icon);
            image.add_css_class("row-icon");
            if !tone.is_empty() {
                image.add_css_class(tone);
            }
            image.set_accessible_role(gtk::AccessibleRole::Presentation);
            line.append(&image);
            let text = widgets::vbox(2);
            text.set_hexpand(true);
            let name = workflows
                .get(&run.workflow_id)
                .map(|w| w.name.clone())
                .unwrap_or_else(|| "Deleted automation".into());
            let title = widgets::label(&name, "settings-option-title");
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            text.append(&title);
            let mut when = vec![
                present::trigger_label(&run.trigger, &names),
                present::relative_time(run.started_at, now),
            ];
            if let Some(end) = run.finished_at {
                when.push(present::duration(end - run.started_at));
            }
            let started = glib::DateTime::from_unix_local(run.started_at)
                .ok()
                .and_then(|d| d.format("%c").ok())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let meta = widgets::label(&when.join(" · "), "settings-option-description");
            meta.set_tooltip_text(Some(&started));
            text.append(&meta);
            line.append(&text);
            line.append(&widgets::tag(status, tone));
            let chevron = gtk::Image::from_icon_name("pan-down-symbolic");
            chevron.add_css_class("dim");
            chevron.set_accessible_role(gtk::AccessibleRole::Presentation);
            line.append(&chevron);
            column.append(&line);
            let steps = widgets::vbox(4);
            steps.add_css_class("run-steps");
            let revealer = gtk::Revealer::builder()
                .child(&steps)
                .transition_duration(if crate::prefs::get().reduce_motion {
                    0
                } else {
                    150
                })
                .build();
            column.append(&revealer);
            row.set_child(Some(&column));
            row.update_property(&[gtk::accessible::Property::Label(&format!(
                "{name}, {status}, {}",
                present::relative_time(run.started_at, now)
            ))]);
            row.set_tooltip_text(Some("Show the steps (Enter)"));
            self.list.append(&row);
            if self.expanded.borrow().contains(&run.id) {
                self.load_steps(run, &steps, &revealer);
            }
        }
        self.stack.set_visible_child_name("list");
    }

    fn toggle(&self, row: &gtk::ListBoxRow) {
        let id = row.widget_name().to_string();
        let Some(revealer) = row
            .child()
            .and_then(|c| c.last_child())
            .and_downcast::<gtk::Revealer>()
        else {
            return;
        };
        if revealer.reveals_child() {
            revealer.set_reveal_child(false);
            self.expanded.borrow_mut().remove(&id);
            return;
        }
        self.expanded.borrow_mut().insert(id.clone());
        let Some(run) = self.runs.borrow().iter().find(|r| r.id == id).cloned() else {
            return;
        };
        if let Some(steps) = revealer.child().and_downcast::<gtk::Box>() {
            self.load_steps(&run, &steps, &revealer);
        }
    }

    fn load_steps(&self, run: &Run, steps: &gtk::Box, revealer: &gtk::Revealer) {
        let workflow = self.workflows.borrow().get(&run.workflow_id).cloned();
        let (steps, revealer) = (steps.clone(), revealer.clone());
        let run_id = run.id.clone();
        rt::spawn(
            async move { service::progress(&run_id).await },
            move |result| {
                while let Some(child) = steps.first_child() {
                    steps.remove(&child);
                }
                let names = |id: &str| {
                    app::get().map_or_else(|| id.to_string(), |app| app.home.borrow().name_of(id))
                };
                match result {
                    Ok(progress) => {
                        for node in progress.nodes {
                            let label = workflow
                                .as_ref()
                                .and_then(|w| w.nodes.iter().find(|n| n.id == node.node_id))
                                .map(|n| present::node_label(&n.kind, &names))
                                .unwrap_or_else(|| "Removed step".into());
                            let (status, _, tone) = present::status(&node.status);
                            let line = widgets::hbox(10);
                            let text = widgets::vbox(1);
                            text.set_hexpand(true);
                            text.append(&widgets::label(&label, ""));
                            if !node.detail.is_empty() && node.status != "skipped" {
                                let detail =
                                    widgets::label(&node.detail, "settings-option-description");
                                detail.set_wrap(true);
                                text.append(&detail);
                            }
                            line.append(&text);
                            line.append(&widgets::tag(status, tone));
                            steps.append(&line);
                        }
                    }
                    Err(error) => steps.append(&widgets::label(&error, "error-text")),
                }
                revealer.set_reveal_child(true);
            },
        );
    }
}
