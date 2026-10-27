use futures_util::future::{BoxFuture, join_all};
use nyx_home::automation::{BUS, DryRunContext, NodeKind, Owner, PATH, Workflow};
use nyx_home::hybrid::{HybridClient, StateEvent};
use rusqlite::{Connection, params};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    process::Command,
    sync::broadcast,
};
use tracing::{error, info, warn};
use zbus::interface;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone)]
struct Engine {
    db: PathBuf,
    running: Arc<Mutex<HashSet<String>>>,
    tasks: Arc<Mutex<HashMap<String, tokio::task::AbortHandle>>>,
    ha_online: Arc<AtomicBool>,
    home_events: broadcast::Sender<StateEvent>,
    window_events: broadcast::Sender<(String, String)>,
}

impl Engine {
    fn open(&self) -> Result<Connection> {
        let db = Connection::open(&self.db)?;
        db.busy_timeout(Duration::from_secs(3))?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS workflows (id TEXT PRIMARY KEY, json TEXT NOT NULL, updated_at INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS runs (id TEXT PRIMARY KEY, workflow_id TEXT NOT NULL, trigger TEXT NOT NULL, started_at INTEGER NOT NULL, finished_at INTEGER, status TEXT NOT NULL, detail TEXT NOT NULL); CREATE INDEX IF NOT EXISTS runs_recent ON runs(started_at DESC); CREATE TABLE IF NOT EXISTS run_nodes (run_id TEXT NOT NULL, node_id TEXT NOT NULL, status TEXT NOT NULL, detail TEXT NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY(run_id,node_id)); CREATE TABLE IF NOT EXISTS event_receipts (workflow_id TEXT NOT NULL, event_id TEXT NOT NULL, received_at INTEGER NOT NULL, PRIMARY KEY(workflow_id,event_id)); CREATE INDEX IF NOT EXISTS event_receipts_age ON event_receipts(received_at);")?;
        Ok(db)
    }

    fn list(&self) -> Result<Vec<Workflow>> {
        let db = self.open()?;
        let mut stmt = db.prepare("SELECT json FROM workflows ORDER BY updated_at DESC")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    fn get(&self, id: &str) -> Result<Option<Workflow>> {
        let db = self.open()?;
        let mut stmt = db.prepare("SELECT json FROM workflows WHERE id = ?1")?;
        let mut rows = stmt.query([id])?;
        match rows.next()? {
            Some(row) => Ok(Some(serde_json::from_str(&row.get::<_, String>(0)?)?)),
            None => Ok(None),
        }
    }

    fn save(&self, workflow: &Workflow) -> Result<()> {
        workflow
            .validate()
            .map_err(|e| format!("Invalid workflow: {e}"))?;
        let db = self.open()?;
        db.execute("INSERT INTO workflows(id,json,updated_at) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET json=excluded.json,updated_at=excluded.updated_at",
            params![workflow.id, serde_json::to_string(workflow)?, now()])?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<bool> {
        let db = self.open()?;
        Ok(db.execute("DELETE FROM workflows WHERE id=?1", [id])? > 0)
    }

    fn history(&self, limit: u32) -> Result<String> {
        let db = self.open()?;
        let mut stmt = db.prepare("SELECT id, workflow_id, trigger, started_at, finished_at, status, detail FROM runs ORDER BY started_at DESC LIMIT ?1")?;
        let records: Vec<serde_json::Value> = stmt.query_map([limit.min(200)], |r| Ok(serde_json::json!({
            "id": r.get::<_,String>(0)?, "workflow_id": r.get::<_,String>(1)?, "trigger": r.get::<_,String>(2)?,
            "started_at": r.get::<_,i64>(3)?, "finished_at": r.get::<_,Option<i64>>(4)?, "status": r.get::<_,String>(5)?, "detail": r.get::<_,String>(6)?
        })))?.collect::<std::result::Result<_,_>>()?;
        Ok(serde_json::to_string(&records)?)
    }

    fn progress(&self, run_id: &str) -> Result<String> {
        let db = self.open()?;
        let status: String =
            db.query_row("SELECT status FROM runs WHERE id=?1", [run_id], |r| {
                r.get(0)
            })?;
        let mut stmt = db.prepare("SELECT node_id,status,detail FROM run_nodes WHERE run_id=?1 ORDER BY updated_at,node_id")?;
        let nodes: Vec<serde_json::Value> = stmt.query_map([run_id], |r| Ok(serde_json::json!({"node_id":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?,"detail":r.get::<_,String>(2)?})))?.collect::<std::result::Result<_,_>>()?;
        Ok(serde_json::json!({"status":status,"nodes":nodes}).to_string())
    }

    fn node_progress(&self, run_id: &str, node_id: &str, status: &str, detail: &str) {
        if let Ok(db) = self.open() {
            let _ = db.execute("INSERT INTO run_nodes(run_id,node_id,status,detail,updated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(run_id,node_id) DO UPDATE SET status=excluded.status,detail=excluded.detail,updated_at=excluded.updated_at", params![run_id,node_id,status,detail,now()]);
        }
    }

    fn reserve_event(&self, workflow_id: &str, event_id: &str) -> Result<bool> {
        let db = self.open()?;
        db.execute(
            "DELETE FROM event_receipts WHERE received_at<?1",
            [now() - 7 * 24 * 3600],
        )?;
        Ok(db.execute("INSERT OR IGNORE INTO event_receipts(workflow_id,event_id,received_at) VALUES(?1,?2,?3)", params![workflow_id,event_id,now()])? == 1)
    }

    fn ingest_event(&self, event: StateEvent) {
        let Ok(workflows) = self.list() else {
            return;
        };
        for workflow in workflows
            .into_iter()
            .filter(|w| w.enabled && w.owner == Owner::HybridDesktop)
        {
            if !workflow.nodes.iter().any(|n| matches!(&n.kind, NodeKind::EntityState { entity_id, state } if entity_id == &event.entity_id && state == &event.state)) { continue; }
            match self.reserve_event(&workflow.id, &event.id) {
                Ok(true) => {
                    if let Err(e) = self.run(workflow, format!("home:{}", event.entity_id)) {
                        warn!("hybrid event rejected: {e}");
                    }
                }
                Ok(false) => {}
                Err(e) => warn!("hybrid receipt unavailable: {e}"),
            }
        }
    }

    fn cancel(&self, run_id: &str) -> Result<bool> {
        let Some(task) = self.tasks.lock().unwrap().remove(run_id) else {
            return Ok(false);
        };
        task.abort();
        let db = self.open()?;
        let workflow_id: String =
            db.query_row("SELECT workflow_id FROM runs WHERE id=?1", [run_id], |r| {
                r.get(0)
            })?;
        db.execute("UPDATE runs SET status='canceled',finished_at=?2,detail='Canceled by user' WHERE id=?1 AND status='running'", params![run_id,now()])?;
        db.execute("UPDATE run_nodes SET status='canceled',detail='Canceled by user' WHERE run_id=?1 AND status='running'", [run_id])?;
        self.running.lock().unwrap().remove(&workflow_id);
        Ok(true)
    }

    fn run(&self, workflow: Workflow, trigger: String) -> Result<String> {
        if workflow.owner == Owner::HomeAssistant {
            return Err("Home Assistant owns this workflow".into());
        }
        if !workflow.enabled {
            return Err("Workflow is disabled".into());
        }
        workflow.validate()?;
        {
            let mut running = self.running.lock().unwrap();
            if !running.insert(workflow.id.clone()) {
                return Err("Workflow is already running".into());
            }
        }
        let run_id = uuid::Uuid::new_v4().to_string();
        let db = match self.open() {
            Ok(db) => db,
            Err(e) => {
                self.running.lock().unwrap().remove(&workflow.id);
                return Err(e);
            }
        };
        if workflow.owner == Owner::HybridDesktop {
            let recent: i64 = match db.query_row(
                "SELECT count(*) FROM runs WHERE trigger LIKE 'home:%' AND started_at>?1",
                [now() - 60],
                |r| r.get(0),
            ) {
                Ok(value) => value,
                Err(e) => {
                    self.running.lock().unwrap().remove(&workflow.id);
                    return Err(e.into());
                }
            };
            if recent >= 20 {
                self.running.lock().unwrap().remove(&workflow.id);
                return Err("Hybrid event rate limit reached".into());
            }
        }
        if let Err(e) = db.execute("INSERT INTO runs(id,workflow_id,trigger,started_at,status,detail) VALUES(?1,?2,?3,?4,'running','')",
            params![run_id, workflow.id, trigger, now()]) {
            self.running.lock().unwrap().remove(&workflow.id);
            return Err(e.into());
        }
        let engine = self.clone();
        let returned_id = run_id.clone();
        let (start_tx, start_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = start_rx.await;
            let trigger_id = workflow
                .nodes
                .iter()
                .find(|n| n.kind.is_trigger())
                .unwrap()
                .id
                .clone();
            let provider = if workflow.owner == Owner::HybridDesktop {
                HybridClient::load().await.ok()
            } else {
                None
            };
            let (status, detail) = if workflow.owner == Owner::HybridDesktop && provider.is_none() {
                (
                    "failed",
                    vec!["Home Assistant credential is unavailable".into()],
                )
            } else {
                run_path(
                    &engine,
                    &workflow,
                    &trigger_id,
                    &run_id,
                    provider.as_ref(),
                    HashMap::new(),
                )
                .await
            };
            if let Ok(db) = engine.open() {
                for node in &workflow.nodes {
                    let _ = db.execute("INSERT OR IGNORE INTO run_nodes(run_id,node_id,status,detail,updated_at) VALUES(?1,?2,'skipped','Path not taken',?3)", params![run_id,node.id,now()]);
                }
                let _ = db.execute(
                    "UPDATE runs SET finished_at=?2,status=?3,detail=?4 WHERE id=?1",
                    params![run_id, now(), status, detail.join("; ")],
                );
            }
            engine.running.lock().unwrap().remove(&workflow.id);
            engine.tasks.lock().unwrap().remove(&run_id);
            info!(workflow=%workflow.id, status, "workflow finished");
        });
        self.tasks
            .lock()
            .unwrap()
            .insert(returned_id.clone(), task.abort_handle());
        let _ = start_tx.send(());
        Ok(returned_id)
    }
}

fn run_path<'a>(
    engine: &'a Engine,
    workflow: &'a Workflow,
    id: &'a str,
    run_id: &'a str,
    provider: Option<&'a HybridClient>,
    mut vars: HashMap<String, String>,
) -> BoxFuture<'a, (&'static str, Vec<String>)> {
    Box::pin(async move {
        let node = workflow.nodes.iter().find(|n| n.id == id).unwrap();
        let started = std::time::Instant::now();
        engine.node_progress(run_id, id, "running", "Executing");
        let mut detail = Vec::new();
        let mut chosen: Option<&str> = None;
        match &node.kind {
            kind if kind.is_trigger() => {
                detail.push(format!("{id}: triggered"));
            }
            NodeKind::AppRunning { class } | NodeKind::BranchAppRunning { class } => {
                match app_running(class).await {
                    Ok(value) => {
                        detail.push(format!("{}: condition {}", id, value));
                        if matches!(node.kind, NodeKind::BranchAppRunning { .. }) {
                            chosen = Some(if value { "true" } else { "false" });
                        } else if !value {
                            engine.node_progress(run_id, id, "skipped", "Condition false");
                            return ("skipped", detail);
                        }
                    }
                    Err(_) => {
                        detail.push(format!("{}: condition unavailable", id));
                        engine.node_progress(
                            run_id,
                            id,
                            "failed",
                            "Application condition unavailable",
                        );
                        return ("failed", detail);
                    }
                }
            }
            NodeKind::EntityIs { entity_id, state } => {
                let result = match provider {
                    Some(client) => client.state(entity_id).await,
                    None => Err("Home Assistant unavailable".into()),
                };
                match result {
                    Ok(actual) if actual == *state => detail.push(format!("{id}: condition true")),
                    Ok(_) => {
                        engine.node_progress(run_id, id, "skipped", "Condition false");
                        return ("skipped", vec![format!("{id}: condition false")]);
                    }
                    Err(e) => {
                        engine.node_progress(run_id, id, "failed", &e);
                        return ("failed", vec![format!("{id}: {e}")]);
                    }
                }
            }
            NodeKind::VariableEquals { name, value } => {
                if vars.get(name) != Some(value) {
                    engine.node_progress(run_id, id, "skipped", "Condition false");
                    return ("skipped", vec![format!("{id}: condition false")]);
                }
                detail.push(format!("{id}: condition true"));
            }
            NodeKind::SetVariable { name, value } => {
                vars.insert(name.clone(), value.clone());
                detail.push(format!("{id}: variable set"));
            }
            NodeKind::Parallel => {
                detail.push(format!("{id}: paths started"));
            }
            NodeKind::Repeat { times } => {
                let child = workflow.edges.iter().find(|e| e.from == id).unwrap();
                let mut handled = false;
                for iteration in 1..=*times {
                    engine.node_progress(
                        run_id,
                        id,
                        "running",
                        &format!("Iteration {iteration}/{times}"),
                    );
                    let (status, child_detail) =
                        run_path(engine, workflow, &child.to, run_id, provider, vars.clone()).await;
                    detail.extend(child_detail);
                    if status == "failed" {
                        engine.node_progress(run_id, id, "failed", "Repeated path failed");
                        return ("failed", detail);
                    }
                    if status == "skipped" {
                        engine.node_progress(run_id, id, "skipped", "Repeated path skipped");
                        return ("skipped", detail);
                    }
                    handled |= status == "completed_with_errors";
                }
                engine.node_progress(
                    run_id,
                    id,
                    "completed",
                    &format!("Completed {times} iterations"),
                );
                return (
                    if handled {
                        "completed_with_errors"
                    } else {
                        "completed"
                    },
                    detail,
                );
            }
            NodeKind::WaitWindowOpened {
                class,
                timeout_seconds,
            } => {
                let mut events = engine.window_events.subscribe();
                let matched = tokio::time::timeout(Duration::from_secs(*timeout_seconds), async {
                    loop {
                        match events.recv().await {
                            Ok((kind, actual))
                                if kind == "window_opened"
                                    && actual.eq_ignore_ascii_case(class) =>
                            {
                                return true;
                            }
                            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => return false,
                        }
                    }
                })
                .await
                .unwrap_or(false);
                if !matched {
                    engine.node_progress(run_id, id, "failed", "Window wait timed out");
                    return ("failed", vec![format!("{id}: window wait timed out")]);
                }
                detail.push(format!("{id}: window opened"));
            }
            NodeKind::WaitEntityState {
                entity_id,
                state,
                timeout_seconds,
            } => {
                let mut events = engine.home_events.subscribe();
                let matched = tokio::time::timeout(Duration::from_secs(*timeout_seconds), async {
                    loop {
                        match events.recv().await {
                            Ok(event) if event.entity_id == *entity_id && event.state == *state => {
                                return true;
                            }
                            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => return false,
                        }
                    }
                })
                .await
                .unwrap_or(false);
                if !matched {
                    engine.node_progress(
                        run_id,
                        id,
                        "failed",
                        "Home Assistant event wait timed out",
                    );
                    return (
                        "failed",
                        vec![format!("{id}: Home Assistant event wait timed out")],
                    );
                }
                detail.push(format!("{id}: entity state observed"));
            }
            NodeKind::Stop => {
                engine.node_progress(run_id, id, "completed", "Stopped path");
                return ("completed", vec![format!("{id}: stopped")]);
            }
            NodeKind::Delay { seconds } => {
                tokio::time::sleep(Duration::from_secs(*seconds)).await;
                detail.push(format!("{}: completed", id));
            }
            kind if kind.is_action() => {
                let policy = workflow.policies.get(id);
                let retries = policy.map_or(0, |p| p.retries);
                let timeout = policy.map_or(10, |p| p.timeout_seconds);
                let mut failure = None;
                for attempt in 0..=retries {
                    let result =
                        tokio::time::timeout(Duration::from_secs(timeout), execute(kind, provider))
                            .await;
                    match result {
                        Ok(Ok(())) => {
                            failure = None;
                            if attempt > 0 {
                                detail.push(format!("{id}: succeeded after {} retries", attempt));
                            }
                            break;
                        }
                        Ok(Err(e)) => failure = Some(e.to_string()),
                        Err(_) => failure = Some("timed out".into()),
                    }
                    if attempt < retries {
                        detail.push(format!(
                            "{id}: retry {} after {}",
                            attempt + 1,
                            failure.as_deref().unwrap_or("failure")
                        ));
                    }
                    if attempt < retries {
                        tokio::time::sleep(Duration::from_millis(200 * (attempt as u64 + 1))).await;
                    }
                }
                if let Some(error) = failure {
                    engine.node_progress(
                        run_id,
                        id,
                        "failed",
                        &format!("{error} · {} ms", started.elapsed().as_millis()),
                    );
                    detail.push(format!("{id}: {error}"));
                    if let Some(edge) = workflow
                        .edges
                        .iter()
                        .find(|e| e.from == id && e.output.as_deref() == Some("error"))
                    {
                        let (child_status, child_detail) =
                            run_path(engine, workflow, &edge.to, run_id, provider, vars).await;
                        detail.extend(child_detail);
                        return (
                            if child_status == "failed" {
                                "failed"
                            } else {
                                "completed_with_errors"
                            },
                            detail,
                        );
                    }
                    return ("failed", detail);
                }
                detail.push(format!("{id}: completed"));
            }
            _ => {
                engine.node_progress(run_id, id, "failed", "Unsupported node");
                return ("failed", vec![format!("{id}: unsupported node")]);
            }
        }
        engine.node_progress(
            run_id,
            id,
            "completed",
            &format!(
                "{} · {} ms",
                detail.last().map(String::as_str).unwrap_or("Completed"),
                started.elapsed().as_millis()
            ),
        );
        let edges: Vec<_> = workflow
            .edges
            .iter()
            .filter(|e| {
                e.from == id
                    && e.output.as_deref() != Some("error")
                    && chosen.is_none_or(|port| e.output.as_deref() == Some(port))
            })
            .collect();
        let results = join_all(
            edges
                .iter()
                .map(|edge| run_path(engine, workflow, &edge.to, run_id, provider, vars.clone())),
        )
        .await;
        let mut failed = false;
        let mut all_skipped = !results.is_empty();
        let mut handled = false;
        for (child_status, child_detail) in results {
            if child_status == "failed" {
                failed = true;
            }
            if child_status == "completed_with_errors" {
                handled = true;
            }
            all_skipped &= child_status == "skipped";
            detail.extend(child_detail);
        }
        let status = if failed {
            "failed"
        } else if handled {
            "completed_with_errors"
        } else if all_skipped {
            "skipped"
        } else {
            "completed"
        };
        (status, detail)
    })
}

async fn app_running(class: &str) -> Result<bool> {
    let clients: serde_json::Value = serde_json::from_str(&hypr_request("j/clients").await?)?;
    Ok(clients.as_array().into_iter().flatten().any(|client| {
        client["class"]
            .as_str()
            .is_some_and(|value| value.eq_ignore_ascii_case(class))
    }))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

async fn execute(action: &NodeKind, provider: Option<&HybridClient>) -> Result<()> {
    if let NodeKind::SetEntity { entity_id, turn_on } = action {
        let domain = entity_id.split('.').next().ok_or("Invalid entity")?;
        let service = if *turn_on { "turn_on" } else { "turn_off" };
        return provider
            .ok_or("Home Assistant unavailable")?
            .call(domain, service, entity_id)
            .await
            .map_err(Into::into);
    }
    if let NodeKind::ActivateScene { entity_id } = action {
        return provider
            .ok_or("Home Assistant unavailable")?
            .call("scene", "turn_on", entity_id)
            .await
            .map_err(Into::into);
    }
    if let NodeKind::SwitchWorkspace { number } = action {
        let result = hypr_request(&format!("dispatch workspace {number}")).await?;
        if result.trim() != "ok" {
            return Err("Hyprland rejected workspace change".into());
        }
        return Ok(());
    }
    let mut command = match action {
        NodeKind::Notify { title, body } => {
            let mut c = Command::new("notify-send");
            c.args(["--app-name=Home", "--", title, body]);
            c
        }
        NodeKind::SetVolume { percent } => {
            let mut c = Command::new("wpctl");
            c.args([
                "set-volume",
                "@DEFAULT_AUDIO_SINK@",
                &format!("{}%", percent),
            ]);
            c
        }
        NodeKind::LaunchApplication { desktop_id } => {
            let mut c = Command::new("gtk-launch");
            c.arg(desktop_id);
            c
        }
        _ => return Err("Trigger cannot execute as an action".into()),
    };
    let output = command.kill_on_drop(true).output().await?;
    if !output.status.success() {
        return Err(format!("Action exited with {}", output.status).into());
    }
    Ok(())
}

async fn hypr_request(request: &str) -> Result<String> {
    let mut socket = hypr_event_socket().ok_or("Hyprland socket unavailable")?;
    socket.set_file_name(".socket.sock");
    tokio::time::timeout(Duration::from_secs(3), async move {
        let mut stream = UnixStream::connect(socket).await?;
        stream.write_all(request.as_bytes()).await?;
        let mut data = Vec::new();
        stream.read_to_end(&mut data).await?;
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(String::from_utf8(data)?)
    })
    .await?
}

struct Service {
    engine: Engine,
}

#[interface(name = "io.github.design_nexus.nyx.Home.Automation1")]
impl Service {
    fn status(&self) -> String {
        let watching = self.engine.list().is_ok_and(|workflows| {
            workflows.iter().any(|w| {
                w.enabled
                    && w.owner == Owner::HybridDesktop
                    && w.nodes.iter().any(|n| {
                        matches!(
                            n.kind,
                            NodeKind::EntityState { .. } | NodeKind::WaitEntityState { .. }
                        )
                    })
            })
        });
        if !watching {
            "ready · Home Assistant events idle".into()
        } else if self.engine.ha_online.load(Ordering::Relaxed) {
            "ready · Home Assistant connected".into()
        } else {
            "ready · Home Assistant offline".into()
        }
    }
    fn list_workflows(&self) -> zbus::fdo::Result<String> {
        let workflows = self.engine.list().map_err(dbus_error)?;
        serde_json::to_string(&workflows).map_err(dbus_error)
    }
    fn save_workflow(&self, json: &str) -> zbus::fdo::Result<String> {
        let workflow: Workflow = serde_json::from_str(json).map_err(dbus_error)?;
        self.engine.save(&workflow).map_err(dbus_error)?;
        Ok(workflow.id)
    }
    fn dry_run_workflow(&self, json: &str, context_json: &str) -> zbus::fdo::Result<String> {
        let workflow: Workflow = serde_json::from_str(json).map_err(dbus_error)?;
        let context: DryRunContext = serde_json::from_str(context_json).map_err(dbus_error)?;
        serde_json::to_string(&workflow.dry_run(&context).map_err(dbus_error)?).map_err(dbus_error)
    }
    fn delete_workflow(&self, id: &str) -> zbus::fdo::Result<bool> {
        self.engine.delete(id).map_err(dbus_error)
    }
    fn set_enabled(&self, id: &str, enabled: bool) -> zbus::fdo::Result<()> {
        let mut workflow = self
            .engine
            .get(id)
            .map_err(dbus_error)?
            .ok_or_else(|| zbus::fdo::Error::Failed("Workflow not found".into()))?;
        workflow.enabled = enabled;
        self.engine.save(&workflow).map_err(dbus_error)
    }
    fn run_workflow(&self, id: &str) -> zbus::fdo::Result<String> {
        let workflow = self
            .engine
            .get(id)
            .map_err(dbus_error)?
            .ok_or_else(|| zbus::fdo::Error::Failed("Workflow not found".into()))?;
        self.engine
            .run(workflow, "manual".into())
            .map_err(dbus_error)
    }
    fn run_draft_workflow(&self, json: &str) -> zbus::fdo::Result<String> {
        let workflow: Workflow = serde_json::from_str(json).map_err(dbus_error)?;
        self.engine
            .run(workflow, "live_test".into())
            .map_err(dbus_error)
    }
    fn history(&self, limit: u32) -> zbus::fdo::Result<String> {
        self.engine.history(limit).map_err(dbus_error)
    }
    fn run_progress(&self, id: &str) -> zbus::fdo::Result<String> {
        self.engine.progress(id).map_err(dbus_error)
    }
    fn cancel_run(&self, id: &str) -> zbus::fdo::Result<bool> {
        self.engine.cancel(id).map_err(dbus_error)
    }
}

fn dbus_error(e: impl std::fmt::Display) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(e.to_string())
}

async fn schedule(engine: Engine) {
    let mut last = HashMap::<String, std::time::Instant>::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tick.tick().await;
        match engine.list() {
            Ok(workflows) => {
                let mut active = HashSet::new();
                for workflow in workflows {
                    if !workflow.enabled || workflow.owner == Owner::HomeAssistant {
                        continue;
                    }
                    if let Some(seconds) = workflow.nodes.iter().find_map(|n| match n.kind {
                        NodeKind::Interval { seconds } => Some(seconds),
                        _ => None,
                    }) {
                        active.insert(workflow.id.clone());
                        let at = last
                            .entry(workflow.id.clone())
                            .or_insert_with(std::time::Instant::now);
                        if at.elapsed() >= Duration::from_secs(seconds) {
                            *at = std::time::Instant::now();
                            if let Err(e) = engine.run(workflow, "interval".into()) {
                                warn!("schedule: {e}");
                            }
                        }
                    }
                }
                last.retain(|id, _| active.contains(id));
            }
            Err(e) => warn!("schedule: {e}"),
        }
    }
}

async fn hypr_events(engine: Engine) {
    let mut windows = HashMap::<String, String>::new();
    loop {
        let socket = hypr_event_socket();
        if let Some(socket) = socket {
            match UnixStream::connect(socket).await {
                Ok(stream) => {
                    info!("connected to Hyprland events");
                    windows.clear();
                    let mut lines = BufReader::new(stream).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let Some((event, data)) = line.split_once(">>") else {
                            continue;
                        };
                        let (kind, class) = match event {
                            "openwindow" => {
                                let mut fields = data.split(',');
                                let address = fields.next().unwrap_or("");
                                let class = fields.next().unwrap_or("").to_string();
                                windows.insert(address.into(), class.clone());
                                ("window_opened", class)
                            }
                            "closewindow" => {
                                ("window_closed", windows.remove(data).unwrap_or_default())
                            }
                            _ => continue,
                        };
                        let _ = engine.window_events.send((kind.into(), class.clone()));
                        if let Ok(workflows) = engine.list() {
                            for workflow in workflows
                                .into_iter()
                                .filter(|w| w.enabled && w.owner != Owner::HomeAssistant)
                            {
                                if workflow.nodes.iter().any(|n| match (&n.kind, kind) {
                                    (
                                        NodeKind::WindowOpened { class: expected },
                                        "window_opened",
                                    )
                                    | (
                                        NodeKind::WindowClosed { class: expected },
                                        "window_closed",
                                    ) => expected.eq_ignore_ascii_case(&class),
                                    _ => false,
                                }) {
                                    let _ = engine.run(workflow, kind.into());
                                }
                            }
                        }
                    }
                }
                Err(e) => warn!("Hyprland events unavailable: {e}"),
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

fn hypr_event_socket() -> Option<PathBuf> {
    let base = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("hypr");
    if let Some(sig) = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE") {
        let socket = base.join(sig).join(".socket2.sock");
        if socket.exists() {
            return Some(socket);
        }
    }
    std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|entry| entry.path().join(".socket2.sock"))
        .find(|path| path.exists())
}

async fn home_events(engine: Engine) {
    let mut last_error = String::new();
    loop {
        let needed = engine.list().is_ok_and(|workflows| {
            workflows.iter().any(|w| {
                w.enabled
                    && w.owner == Owner::HybridDesktop
                    && w.nodes.iter().any(|n| {
                        matches!(
                            n.kind,
                            NodeKind::EntityState { .. } | NodeKind::WaitEntityState { .. }
                        )
                    })
            })
        });
        if !needed {
            engine.ha_online.store(false, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_secs(15)).await;
            continue;
        }
        match HybridClient::load().await {
            Ok(client) => {
                let listener = engine.clone();
                let stream = engine.home_events.clone();
                let online = engine.ha_online.clone();
                if let Err(error) = client
                    .listen_events(online, move |event| {
                        let _ = stream.send(event.clone());
                        listener.ingest_event(event);
                    })
                    .await
                    && error != last_error
                {
                    warn!("Home Assistant events unavailable: {error}");
                    last_error = error;
                }
            }
            Err(error) => {
                if error != last_error {
                    warn!("Home Assistant events unavailable: {error}");
                    last_error = error;
                }
            }
        }
        engine.ha_online.store(false, Ordering::Relaxed);
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nyx_homed=info".into()),
        )
        .init();
    nyx_core::app::init(nyx_core::app::App {
        slug: "nyx-home",
        name: "Home",
    });
    let dir = nyx_core::paths::data_dir();
    std::fs::create_dir_all(&dir)?;
    let engine = Engine {
        db: dir.join("automation.db"),
        running: Arc::new(Mutex::new(HashSet::new())),
        tasks: Arc::new(Mutex::new(HashMap::new())),
        ha_online: Arc::new(AtomicBool::new(false)),
        home_events: broadcast::channel(256).0,
        window_events: broadcast::channel(256).0,
    };
    let db = engine.open()?;
    db.execute("UPDATE runs SET finished_at=?1,status='interrupted',detail='Service stopped during execution' WHERE status='running'", [now()])?;
    db.execute("UPDATE run_nodes SET status='interrupted',detail='Service stopped during execution' WHERE status='running'", [])?;
    drop(db);
    let _connection = zbus::connection::Builder::session()?
        .name(BUS)?
        .serve_at(
            PATH,
            Service {
                engine: engine.clone(),
            },
        )?
        .build()
        .await?;
    if let Ok(workflows) = engine.list() {
        for workflow in workflows.into_iter().filter(|w| {
            w.enabled
                && w.owner != Owner::HomeAssistant
                && w.nodes.iter().any(|n| matches!(n.kind, NodeKind::Startup))
        }) {
            if let Err(e) = engine.run(workflow, "startup".into()) {
                warn!("startup: {e}");
            }
        }
    }
    tokio::spawn(schedule(engine.clone()));
    tokio::spawn(hypr_events(engine.clone()));
    tokio::spawn(home_events(engine));
    info!("desktop automation service ready");
    tokio::signal::ctrl_c().await.map_err(|e| {
        error!("signal: {e}");
        e
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_receipts_survive_engine_recreation() {
        let path =
            std::env::temp_dir().join(format!("nyx-home-receipts-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        let db = path.join("automation.db");
        let make_engine = || Engine {
            db: db.clone(),
            running: Arc::new(Mutex::new(HashSet::new())),
            tasks: Arc::new(Mutex::new(HashMap::new())),
            ha_online: Arc::new(AtomicBool::new(false)),
            home_events: broadcast::channel(8).0,
            window_events: broadcast::channel(8).0,
        };
        assert!(make_engine().reserve_event("workflow", "event-1").unwrap());
        assert!(!make_engine().reserve_event("workflow", "event-1").unwrap());
        assert!(make_engine().reserve_event("other", "event-1").unwrap());
        std::fs::remove_dir_all(path).unwrap();
    }
}
