use nixe_home::automation::{BUS, NodeKind, PATH, Workflow};
use rusqlite::{Connection, params};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::UnixStream,
    process::Command,
};
use tracing::{error, info, warn};
use zbus::interface;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone)]
struct Engine {
    db: PathBuf,
    running: Arc<Mutex<HashSet<String>>>,
}

impl Engine {
    fn open(&self) -> Result<Connection> {
        let db = Connection::open(&self.db)?;
        db.busy_timeout(Duration::from_secs(3))?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS workflows (id TEXT PRIMARY KEY, json TEXT NOT NULL, updated_at INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS runs (id TEXT PRIMARY KEY, workflow_id TEXT NOT NULL, trigger TEXT NOT NULL, started_at INTEGER NOT NULL, finished_at INTEGER, status TEXT NOT NULL, detail TEXT NOT NULL); CREATE INDEX IF NOT EXISTS runs_recent ON runs(started_at DESC);")?;
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

    fn run(&self, workflow: Workflow, trigger: String) -> Result<String> {
        if !workflow.enabled {
            return Err("Workflow is disabled".into());
        }
        let order = workflow.validate()?;
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
        if let Err(e) = db.execute("INSERT INTO runs(id,workflow_id,trigger,started_at,status,detail) VALUES(?1,?2,?3,?4,'running','')",
            params![run_id, workflow.id, trigger, now()]) {
            self.running.lock().unwrap().remove(&workflow.id);
            return Err(e.into());
        }
        let engine = self.clone();
        let returned_id = run_id.clone();
        tokio::spawn(async move {
            let mut detail = Vec::new();
            let mut status = "completed";
            for id in order.iter().skip(1) {
                let node = workflow.nodes.iter().find(|n| &n.id == id).unwrap();
                match tokio::time::timeout(Duration::from_secs(10), execute(&node.kind)).await {
                    Ok(Ok(())) => detail.push(format!("{}: completed", node.id)),
                    Ok(Err(e)) => {
                        detail.push(format!("{}: {}", node.id, e));
                        status = "failed";
                        break;
                    }
                    Err(_) => {
                        detail.push(format!("{}: timed out", node.id));
                        status = "failed";
                        break;
                    }
                }
            }
            if let Ok(db) = engine.open() {
                let _ = db.execute(
                    "UPDATE runs SET finished_at=?2,status=?3,detail=?4 WHERE id=?1",
                    params![run_id, now(), status, detail.join("; ")],
                );
            }
            engine.running.lock().unwrap().remove(&workflow.id);
            info!(workflow=%workflow.id, status, "workflow finished");
        });
        Ok(returned_id)
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

async fn execute(action: &NodeKind) -> Result<()> {
    let mut command = match action {
        NodeKind::Notify { title, body } => {
            let mut c = Command::new("notify-send");
            c.args(["--app-name=Home", "--", title, body]);
            c
        }
        NodeKind::SwitchWorkspace { number } => {
            let mut c = Command::new("hyprctl");
            c.args(["dispatch", "workspace", &number.to_string()]);
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

struct Service {
    engine: Engine,
}

#[interface(name = "io.github.design_nexus.nixe.Home.Automation1")]
impl Service {
    fn status(&self) -> String {
        "ready".into()
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
    fn history(&self, limit: u32) -> zbus::fdo::Result<String> {
        self.engine.history(limit).map_err(dbus_error)
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
                    if !workflow.enabled {
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
                        if let Ok(workflows) = engine.list() {
                            for workflow in workflows.into_iter().filter(|w| w.enabled) {
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

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "nixe_homed=info".into()),
        )
        .init();
    nixe_core::app::init(nixe_core::app::App {
        slug: "nixe-home",
        name: "Home",
    });
    let dir = nixe_core::paths::data_dir();
    std::fs::create_dir_all(&dir)?;
    let engine = Engine {
        db: dir.join("automation.db"),
        running: Arc::new(Mutex::new(HashSet::new())),
    };
    let db = engine.open()?;
    db.execute("UPDATE runs SET finished_at=?1,status='interrupted',detail='Service stopped during execution' WHERE status='running'", [now()])?;
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
        for workflow in workflows
            .into_iter()
            .filter(|w| w.enabled && w.nodes.iter().any(|n| matches!(n.kind, NodeKind::Startup)))
        {
            if let Err(e) = engine.run(workflow, "startup".into()) {
                warn!("startup: {e}");
            }
        }
    }
    tokio::spawn(schedule(engine.clone()));
    tokio::spawn(hypr_events(engine));
    info!("desktop automation service ready");
    tokio::signal::ctrl_c().await.map_err(|e| {
        error!("signal: {e}");
        e
    })?;
    Ok(())
}
