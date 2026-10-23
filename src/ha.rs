//! Home Assistant transport and normalized entities.
use async_channel::Sender;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub entity_id: String,
    pub state: String,
    #[serde(default)]
    pub attributes: Value,
    #[serde(skip)]
    pub area_name: Option<String>,
    #[serde(skip)]
    pub device_name: Option<String>,
}

impl Entity {
    pub fn name(&self) -> String {
        self.attributes
            .get("friendly_name")
            .and_then(Value::as_str)
            .unwrap_or(&self.entity_id)
            .to_string()
    }
    pub fn domain(&self) -> &str {
        self.entity_id.split_once('.').map(|v| v.0).unwrap_or("")
    }
}

#[derive(Debug, Clone)]
pub enum Update {
    Connected(Inventory),
    Entity(Entity),
    Removed(String),
    Status(String),
}

#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub entities: Vec<Entity>,
    pub area_count: usize,
    pub device_count: usize,
    pub metadata: std::sync::Arc<HashMap<String, Metadata>>,
}

#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub area_name: Option<String>,
    pub device_name: Option<String>,
}

pub async fn fetch_states(base: &str, token: &str) -> Result<Vec<Entity>, String> {
    let url = format!("{}/api/states", base.trim_end_matches('/'));
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|_| "Could not initialize the Home Assistant connection.".to_string())?
        .get(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| {
            "Could not reach Home Assistant. Check the URL and network connection.".to_string()
        })?;
    if !response.status().is_success() {
        return Err(format!(
            "Home Assistant returned HTTP {}.",
            response.status()
        ));
    }
    response
        .json::<Vec<Entity>>()
        .await
        .map_err(|_| "Home Assistant returned an unreadable entity list.".into())
}

pub async fn fetch_inventory(base: &str, token: &str) -> Result<Inventory, String> {
    let mut entities = fetch_states(base, token).await?;
    let registry = tokio::time::timeout(
        std::time::Duration::from_secs(6),
        fetch_registry(base, token),
    )
    .await;
    let (metadata, area_count, device_count) =
        registry.ok().and_then(Result::ok).unwrap_or_default();
    for entity in &mut entities {
        if let Some(meta) = metadata.get(&entity.entity_id) {
            entity.area_name = meta.area_name.clone();
            entity.device_name = meta.device_name.clone();
        }
    }
    Ok(Inventory {
        entities,
        area_count,
        device_count,
        metadata: std::sync::Arc::new(metadata),
    })
}

async fn fetch_registry(
    base: &str,
    token: &str,
) -> Result<(HashMap<String, Metadata>, usize, usize), String> {
    let mut url = url::Url::parse(base).map_err(|_| "Invalid URL".to_string())?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme)
        .map_err(|_| "Invalid URL".to_string())?;
    url.set_path("/api/websocket");
    let (mut socket, _) = tokio_tungstenite::connect_async(url.as_str())
        .await
        .map_err(|_| "Registry unavailable".to_string())?;
    read_json(&mut socket).await?; // auth_required
    socket
        .send(Message::Text(
            json!({"type":"auth","access_token":token})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| "Registry unavailable".to_string())?;
    let auth = read_json(&mut socket).await?;
    if auth["type"] != "auth_ok" {
        return Err("Registry authentication failed".into());
    }
    let areas = registry_call(&mut socket, 1, "config/area_registry/list")
        .await
        .unwrap_or_default();
    let devices = registry_call(&mut socket, 2, "config/device_registry/list")
        .await
        .unwrap_or_default();
    let entity_registry = registry_call(&mut socket, 3, "config/entity_registry/list")
        .await
        .unwrap_or_default();
    let mut area_names = HashMap::new();
    for area in areas.as_array().into_iter().flatten() {
        if let (Some(id), Some(name)) = (area["area_id"].as_str(), area["name"].as_str()) {
            area_names.insert(id.to_string(), name.to_string());
        }
    }
    let mut device_data = HashMap::new();
    for device in devices.as_array().into_iter().flatten() {
        if let Some(id) = device["id"].as_str() {
            device_data.insert(
                id.to_string(),
                (
                    device["name_by_user"]
                        .as_str()
                        .or_else(|| device["name"].as_str())
                        .map(str::to_string),
                    device["area_id"].as_str().map(str::to_string),
                ),
            );
        }
    }
    let mut metadata = HashMap::new();
    for item in entity_registry.as_array().into_iter().flatten() {
        let Some(entity_id) = item["entity_id"].as_str() else {
            continue;
        };
        let device = item["device_id"]
            .as_str()
            .and_then(|id| device_data.get(id));
        let area_id = item["area_id"]
            .as_str()
            .map(str::to_string)
            .or_else(|| device.and_then(|d| d.1.clone()));
        metadata.insert(
            entity_id.to_string(),
            Metadata {
                area_name: area_id.as_ref().and_then(|id| area_names.get(id)).cloned(),
                device_name: device.and_then(|d| d.0.clone()),
            },
        );
    }
    Ok((metadata, area_names.len(), device_data.len()))
}

async fn read_json<S>(socket: &mut WebSocketStream<S>) -> Result<Value, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let message = socket
        .next()
        .await
        .and_then(Result::ok)
        .and_then(|m| m.into_text().ok())
        .ok_or_else(|| "WebSocket closed".to_string())?;
    serde_json::from_str(&message).map_err(|_| "Invalid WebSocket response".into())
}

async fn registry_call<S>(
    socket: &mut WebSocketStream<S>,
    id: u64,
    kind: &str,
) -> Result<Value, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    socket
        .send(Message::Text(
            json!({"id":id,"type":kind}).to_string().into(),
        ))
        .await
        .map_err(|_| "Registry request failed".to_string())?;
    loop {
        let response = read_json(socket).await?;
        if response["id"].as_u64() == Some(id) && response["success"] == true {
            return Ok(response["result"].clone());
        }
        if response["id"].as_u64() == Some(id) {
            return Err("Registry request rejected".into());
        }
    }
}

pub async fn call_service(base: &str, token: &str, entity: &Entity) -> Result<(), String> {
    let (domain, service) = match entity.domain() {
        "light" | "switch" | "input_boolean" | "fan" => (
            entity.domain(),
            if entity.state == "on" {
                "turn_off"
            } else {
                "turn_on"
            },
        ),
        "lock" => (
            "lock",
            if entity.state == "locked" {
                "unlock"
            } else {
                "lock"
            },
        ),
        _ => return Err("This entity does not have a supported quick control.".into()),
    };
    let url = format!(
        "{}/api/services/{domain}/{service}",
        base.trim_end_matches('/')
    );
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(token)
        .json(&json!({"entity_id": entity.entity_id}))
        .send()
        .await
        .map_err(|_| "The Home Assistant service call failed.".to_string())?;
    response
        .status()
        .is_success()
        .then_some(())
        .ok_or_else(|| format!("Home Assistant returned HTTP {}.", response.status()))
}

pub fn start_live_updates(
    base: String,
    token: String,
    sender: Sender<Update>,
    cancel: async_channel::Receiver<()>,
    metadata: std::sync::Arc<HashMap<String, Metadata>>,
) {
    std::thread::spawn(move || {
        let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        runtime.block_on(async move {
            loop {
                if cancel.is_closed() {
                    return;
                }
                if sender
                    .send(Update::Status("Connecting to Home Assistant…".into()))
                    .await
                    .is_err()
                {
                    return;
                }
                tokio::select! {
                    _ = cancel.recv() => return,
                    result = websocket_loop(&base, &token, &sender, &metadata) => match result {
                        Ok(()) => {},
                        Err(message) => { let _ = sender.send(Update::Status(message)).await; }
                    }
                }
                tokio::select! {
                    _ = cancel.recv() => return,
                    _ = tokio::time::sleep(std::time::Duration::from_secs(4)) => {}
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_labels_use_home_assistant_friendly_name() {
        let entity: Entity = serde_json::from_value(json!({
            "entity_id": "light.office",
            "state": "on",
            "attributes": {"friendly_name": "Office lights"}
        }))
        .unwrap();
        assert_eq!(entity.name(), "Office lights");
        assert_eq!(entity.domain(), "light");
    }

    #[test]
    fn missing_friendly_name_falls_back_to_entity_id() {
        let entity: Entity = serde_json::from_value(json!({
            "entity_id": "sensor.temperature", "state": "21.5", "attributes": {}
        }))
        .unwrap();
        assert_eq!(entity.name(), "sensor.temperature");
    }

    #[tokio::test]
    async fn fetch_states_accepts_a_mocked_home_assistant_response() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let read = stream.read(&mut request).await.unwrap();
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-token")
            );
            let body = r#"[{"entity_id":"light.office","state":"on","attributes":{"friendly_name":"Office lights"}}]"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let entities = fetch_states(&format!("http://{address}"), "test-token")
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name(), "Office lights");
    }
}

async fn websocket_loop(
    base: &str,
    token: &str,
    sender: &Sender<Update>,
    metadata: &HashMap<String, Metadata>,
) -> Result<(), String> {
    let mut url =
        url::Url::parse(base).map_err(|_| "Enter a valid Home Assistant URL.".to_string())?;
    let scheme = match url.scheme() {
        "http" => "ws",
        "https" => "wss",
        "ws" => "ws",
        "wss" => "wss",
        _ => return Err("Use an HTTP or HTTPS Home Assistant URL.".into()),
    };
    url.set_scheme(scheme)
        .map_err(|_| "The Home Assistant URL is invalid.".to_string())?;
    url.set_path("/api/websocket");
    let (mut socket, _) = tokio_tungstenite::connect_async(url.as_str())
        .await
        .map_err(|_| "Home Assistant is offline. Retrying…".to_string())?;
    let hello = socket
        .next()
        .await
        .and_then(Result::ok)
        .and_then(|m| m.into_text().ok())
        .ok_or_else(|| "Home Assistant closed the WebSocket connection.".to_string())?;
    if serde_json::from_str::<Value>(&hello)
        .ok()
        .and_then(|v| v["type"].as_str().map(str::to_owned))
        .as_deref()
        != Some("auth_required")
    {
        return Err("Home Assistant sent an unexpected WebSocket response.".into());
    }
    socket
        .send(Message::Text(
            json!({"type":"auth","access_token":token})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| "Could not authenticate with Home Assistant.".to_string())?;
    let auth = socket
        .next()
        .await
        .and_then(Result::ok)
        .and_then(|m| m.into_text().ok())
        .ok_or_else(|| "Home Assistant closed the WebSocket connection.".to_string())?;
    if serde_json::from_str::<Value>(&auth)
        .ok()
        .and_then(|v| v["type"].as_str().map(str::to_owned))
        .as_deref()
        != Some("auth_ok")
    {
        return Err(
            "Home Assistant rejected the credential. Reconnect with a valid sign-in.".into(),
        );
    }
    socket
        .send(Message::Text(
            json!({"id":1,"type":"subscribe_events","event_type":"state_changed"})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| "Could not subscribe to Home Assistant updates.".to_string())?;
    let _ = sender
        .send(Update::Status(
            "Connected · listening for live updates".into(),
        ))
        .await;
    while let Some(message) = socket.next().await {
        let message = message.map_err(|_| "Connection lost. Retrying…".to_string())?;
        let Ok(text) = message.into_text() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if value["type"] != "event" {
            continue;
        }
        if value["event"]["data"]["new_state"].is_null() {
            if let Some(entity_id) = value["event"]["data"]["entity_id"].as_str()
                && sender
                    .send(Update::Removed(entity_id.to_string()))
                    .await
                    .is_err()
            {
                return Ok(());
            }
            continue;
        }
        let Some(state) = value["event"]["data"]["new_state"].as_object() else {
            continue;
        };
        let Ok(entity) = serde_json::from_value::<Entity>(Value::Object(state.clone())) else {
            continue;
        };
        let mut entity = entity;
        if let Some(meta) = metadata.get(&entity.entity_id) {
            entity.area_name = meta.area_name.clone();
            entity.device_name = meta.device_name.clone();
        }
        if sender.send(Update::Entity(entity)).await.is_err() {
            return Ok(());
        }
    }
    Err("Connection lost. Retrying…".into())
}
