//! The desktop automation service (`nyx-homed`) over the session bus. One
//! connection is shared by every call; calls run on the I/O runtime.
use nyx_home::automation::{BUS, INTERFACE, PATH, Workflow};
use serde::{Serialize, de::DeserializeOwned};
use tokio::sync::OnceCell;
use zbus::zvariant::{DynamicType, Type};

async fn proxy() -> Result<zbus::Proxy<'static>, String> {
    static CONNECTION: OnceCell<zbus::Connection> = OnceCell::const_new();
    let connection = CONNECTION
        .get_or_try_init(zbus::Connection::session)
        .await
        .map_err(|_| "The session bus is unavailable.".to_string())?;
    zbus::Proxy::new(connection, BUS, PATH, INTERFACE)
        .await
        .map_err(|e| e.to_string())
}

async fn call<B, R>(method: &'static str, body: &B) -> Result<R, String>
where
    B: Serialize + DynamicType,
    R: DeserializeOwned + Type,
{
    proxy()
        .await?
        .call(method, body)
        .await
        .map_err(|e| match e {
            zbus::Error::MethodError(name, _, _)
                if name.as_str() == "org.freedesktop.DBus.Error.ServiceUnknown" =>
            {
                "The desktop automation service isn't running.".to_string()
            }
            zbus::Error::MethodError(_, Some(detail), _) => detail,
            other => other.to_string(),
        })
}

pub async fn status() -> Result<String, String> {
    call("Status", &()).await
}

pub async fn workflows() -> Result<Vec<Workflow>, String> {
    let json: String = call("ListWorkflows", &()).await?;
    serde_json::from_str(&json).map_err(|_| "The service sent unreadable workflows.".into())
}

pub async fn save(workflow: &Workflow) -> Result<(), String> {
    let json = serde_json::to_string(workflow).map_err(|e| e.to_string())?;
    call::<_, String>("SaveWorkflow", &json).await.map(|_| ())
}

pub async fn delete(id: &str) -> Result<bool, String> {
    call("DeleteWorkflow", &id).await
}

pub async fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    call("SetEnabled", &(id, enabled)).await
}

pub async fn run(id: &str) -> Result<String, String> {
    call("RunWorkflow", &id).await
}

pub async fn run_draft(workflow: &Workflow) -> Result<String, String> {
    let json = serde_json::to_string(workflow).map_err(|e| e.to_string())?;
    call("RunDraftWorkflow", &json).await
}

pub async fn history(limit: u32) -> Result<Vec<Run>, String> {
    let json: String = call("History", &limit).await?;
    serde_json::from_str(&json).map_err(|_| "The service sent an unreadable history.".into())
}

pub async fn progress(run_id: &str) -> Result<Progress, String> {
    let json: String = call("RunProgress", &run_id).await?;
    serde_json::from_str(&json).map_err(|_| "The service sent unreadable progress.".into())
}

pub async fn cancel(run_id: &str) -> Result<bool, String> {
    call("CancelRun", &run_id).await
}

#[derive(Clone, Debug, serde::Deserialize, PartialEq)]
pub struct Run {
    pub id: String,
    pub workflow_id: String,
    pub trigger: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub status: String,
    pub detail: String,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct Progress {
    pub status: String,
    pub nodes: Vec<NodeProgress>,
}

#[derive(Clone, Debug, serde::Deserialize)]
pub struct NodeProgress {
    pub node_id: String,
    pub status: String,
    pub detail: String,
}
