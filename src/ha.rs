//! Home Assistant transport and normalized entities.
use crate::home_features::{self, RobotAction};
use async_channel::Sender;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, sync::OnceLock, time::Duration};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entity {
    pub entity_id: String,
    pub state: String,
    #[serde(default)]
    pub attributes: Value,
    #[serde(default)]
    pub last_changed: Option<String>,
    #[serde(skip)]
    pub area_name: Option<String>,
    #[serde(skip)]
    pub device_name: Option<String>,
    #[serde(skip)]
    pub device_id: Option<String>,
    /// `config` or `diagnostic` for entities that aren't a device's main function.
    #[serde(skip)]
    pub entity_category: Option<String>,
    /// Hidden by the user or an integration in Home Assistant.
    #[serde(skip)]
    pub hidden: bool,
    /// The integration that provides it (`group` for a light group).
    #[serde(skip)]
    pub platform: Option<String>,
    /// The config entry it comes from, for groups made in the UI.
    #[serde(skip)]
    pub config_entry_id: Option<String>,
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
    fn apply(&mut self, meta: &Metadata) {
        self.area_name = meta.area_name.clone();
        self.device_name = meta.device_name.clone();
        self.device_id = meta.device_id.clone();
        self.entity_category = meta.entity_category.clone();
        self.hidden = meta.hidden;
        self.platform = meta.platform.clone();
        self.config_entry_id = meta.config_entry_id.clone();
    }

    /// A light group: a light that stands for other lights.
    pub fn members(&self) -> Option<Vec<String>> {
        if self.domain() != "light" {
            return None;
        }
        let list: Vec<String> = self.attributes["entity_id"]
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect();
        (!list.is_empty()).then_some(list)
    }
}

#[derive(Debug, Clone)]
pub enum Update {
    Connected(Inventory),
    Entity(Entity),
    Removed(String),
    /// The live event stream is subscribed.
    Online,
    /// The live event stream dropped; it retries on its own.
    Offline(String),
}

#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub entities: Vec<Entity>,
    pub areas: Vec<Area>,
    pub device_count: usize,
    pub devices: HashMap<String, DeviceInfo>,
    pub info: Info,
    pub metadata: Arc<HashMap<String, Metadata>>,
}

/// A device from Home Assistant's device registry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub area_name: Option<String>,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    /// `service` for integrations that aren't physical devices.
    pub entry_type: Option<String>,
    /// Home Assistant label IDs (`plugged_in`).
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Metadata {
    pub area_name: Option<String>,
    pub device_name: Option<String>,
    pub device_id: Option<String>,
    pub entity_category: Option<String>,
    pub hidden: bool,
    pub platform: Option<String>,
    pub config_entry_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Area {
    pub id: String,
    pub name: String,
}

/// What Home Assistant says about itself (`/api/config`).
#[derive(Debug, Clone, Default)]
pub struct Info {
    pub version: String,
    pub location_name: String,
}

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(12))
            .connect_timeout(Duration::from_secs(6))
            .build()
            .unwrap_or_default()
    })
}

fn api(base: &str, path: &str) -> String {
    format!("{}/api/{path}", base.trim_end_matches('/'))
}

fn http_error(status: reqwest::StatusCode) -> String {
    match status.as_u16() {
        401 => "Home Assistant rejected the token. Replace it in Connections.".into(),
        403 => "This Home Assistant user isn't allowed to do that.".into(),
        404 => "Home Assistant doesn't know that entity or action.".into(),
        code => format!("Home Assistant returned an error (HTTP {code})."),
    }
}

/// Why a connection check failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    Unreachable,
    Rejected,
    Other(String),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable => f.write_str(
                "Couldn't reach Home Assistant at that address. Check the URL and your network.",
            ),
            Self::Rejected => f.write_str(
                "Home Assistant rejected this token. Create a new long-lived access token and paste it here.",
            ),
            Self::Other(message) => f.write_str(message),
        }
    }
}

/// Check that `base` is a Home Assistant server that accepts `token`.
pub async fn check(base: &str, token: &str) -> Result<Info, CheckError> {
    let response = client()
        .get(api(base, "config"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| CheckError::Unreachable)?;
    match response.status().as_u16() {
        200 => {}
        401 | 403 => return Err(CheckError::Rejected),
        _ => return Err(CheckError::Other(http_error(response.status()))),
    }
    let config: Value = response.json().await.map_err(|_| {
        CheckError::Other("That address answered, but not like Home Assistant.".into())
    })?;
    if config["version"].as_str().is_none() {
        return Err(CheckError::Other(
            "That address answered, but not like Home Assistant.".into(),
        ));
    }
    Ok(Info {
        version: config["version"].as_str().unwrap_or_default().into(),
        location_name: config["location_name"].as_str().unwrap_or_default().into(),
    })
}

pub async fn fetch_states(base: &str, token: &str) -> Result<Vec<Entity>, String> {
    let response = client()
        .get(api(base, "states"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| {
            "Could not reach Home Assistant. Check the URL and network connection.".to_string()
        })?;
    if !response.status().is_success() {
        return Err(http_error(response.status()));
    }
    response
        .json::<Vec<Entity>>()
        .await
        .map_err(|_| "Home Assistant returned an unreadable entity list.".into())
}

pub async fn fetch_inventory(base: &str, token: &str) -> Result<Inventory, String> {
    let info = check(base, token).await.map_err(|e| e.to_string())?;
    let mut entities = fetch_states(base, token).await?;
    let registry = tokio::time::timeout(Duration::from_secs(8), fetch_registry(base, token)).await;
    let (metadata, areas, devices) = registry.ok().and_then(Result::ok).unwrap_or_default();
    for entity in &mut entities {
        if let Some(meta) = metadata.get(&entity.entity_id) {
            entity.apply(meta);
        }
    }
    Ok(Inventory {
        entities,
        areas,
        device_count: devices.len(),
        devices,
        info,
        metadata: Arc::new(metadata),
    })
}

fn websocket_url(base: &str) -> Result<url::Url, String> {
    let mut url =
        url::Url::parse(base).map_err(|_| "Enter a valid Home Assistant URL.".to_string())?;
    let scheme = match url.scheme() {
        "http" | "ws" => "ws",
        "https" | "wss" => "wss",
        _ => return Err("Use an HTTP or HTTPS Home Assistant URL.".into()),
    };
    url.set_scheme(scheme)
        .map_err(|_| "The Home Assistant URL is invalid.".to_string())?;
    url.set_path("/api/websocket");
    Ok(url)
}

async fn authenticate<S>(socket: &mut WebSocketStream<S>, token: &str) -> Result<(), String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    if read_json(socket).await?["type"] != "auth_required" {
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
    if read_json(socket).await?["type"] != "auth_ok" {
        return Err("Home Assistant rejected the token. Replace it in Connections.".into());
    }
    Ok(())
}

async fn fetch_registry(
    base: &str,
    token: &str,
) -> Result<
    (
        HashMap<String, Metadata>,
        Vec<Area>,
        HashMap<String, DeviceInfo>,
    ),
    String,
> {
    let (mut socket, _) = tokio_tungstenite::connect_async(websocket_url(base)?.as_str())
        .await
        .map_err(|_| "Registry unavailable".to_string())?;
    authenticate(&mut socket, token).await?;
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
    let mut area_list = Vec::new();
    for area in areas.as_array().into_iter().flatten() {
        if let (Some(id), Some(name)) = (area["area_id"].as_str(), area["name"].as_str()) {
            area_names.insert(id.to_string(), name.to_string());
            area_list.push(Area {
                id: id.to_string(),
                name: name.to_string(),
            });
        }
    }
    area_list.sort_by_key(|a| a.name.to_lowercase());
    let mut device_data = HashMap::new();
    let mut device_info = HashMap::new();
    for device in devices.as_array().into_iter().flatten() {
        if let Some(id) = device["id"].as_str() {
            let name = device["name_by_user"]
                .as_str()
                .or_else(|| device["name"].as_str())
                .map(str::to_string);
            let area_id = device["area_id"].as_str().map(str::to_string);
            device_info.insert(
                id.to_string(),
                DeviceInfo {
                    id: id.to_string(),
                    name: name.clone().unwrap_or_default(),
                    area_name: area_id.as_ref().and_then(|a| area_names.get(a)).cloned(),
                    manufacturer: device["manufacturer"].as_str().map(str::to_string),
                    model: device["model"].as_str().map(str::to_string),
                    entry_type: device["entry_type"].as_str().map(str::to_string),
                    labels: device["labels"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                },
            );
            device_data.insert(id.to_string(), (name, area_id));
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
                device_id: item["device_id"].as_str().map(str::to_string),
                entity_category: item["entity_category"].as_str().map(str::to_string),
                hidden: !item["hidden_by"].is_null(),
                platform: item["platform"].as_str().map(str::to_string),
                config_entry_id: item["config_entry_id"].as_str().map(str::to_string),
            },
        );
    }
    Ok((metadata, area_list, device_info))
}

async fn read_json<S>(socket: &mut WebSocketStream<S>) -> Result<Value, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let message = tokio::time::timeout(Duration::from_secs(12), socket.next())
        .await
        .map_err(|_| "Home Assistant stopped answering.".to_string())?
        .and_then(Result::ok)
        .and_then(|m| m.into_text().ok())
        .ok_or_else(|| "Home Assistant closed the WebSocket connection.".to_string())?;
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

/// The actions the app may call, by domain. Anything else is refused before
/// a request is made.
pub fn allowed(domain: &str, service: &str) -> bool {
    matches!(
        (domain, service),
        (
            "light" | "switch" | "fan" | "input_boolean" | "automation" | "siren" | "humidifier",
            "turn_on" | "turn_off"
        ) | ("lock", "lock" | "unlock")
            | ("cover", "open_cover" | "close_cover" | "stop_cover")
            | (
                "climate",
                "set_temperature" | "set_hvac_mode" | "set_fan_mode"
            )
            | ("fan", "set_preset_mode" | "set_percentage")
            | (
                "media_player",
                "media_play_pause" | "volume_set" | "select_source" | "turn_on" | "turn_off"
            )
            | ("scene" | "script", "turn_on")
            | ("button" | "input_button", "press")
            | ("select" | "input_select", "select_option")
            | ("number" | "input_number", "set_value")
            | (
                "alarm_control_panel",
                "alarm_arm_home" | "alarm_arm_away" | "alarm_arm_night" | "alarm_disarm"
            )
    )
}

/// Call a Home Assistant action on one entity, with extra service data.
pub async fn call(
    base: &str,
    token: &str,
    entity_id: &str,
    service: &str,
    data: Value,
) -> Result<(), String> {
    let domain = entity_id.split_once('.').map(|v| v.0).unwrap_or("");
    if !allowed(domain, service) {
        return Err("This device doesn't offer that control.".into());
    }
    let mut body = match data {
        Value::Object(map) => map,
        _ => Default::default(),
    };
    body.insert("entity_id".into(), entity_id.into());
    let response = client()
        .post(api(base, &format!("services/{domain}/{service}")))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|_| "Couldn't reach Home Assistant. The command wasn't sent.".to_string())?;
    response
        .status()
        .is_success()
        .then_some(())
        .ok_or_else(|| http_error(response.status()))
}

/// Call a light action on every light in an area.
pub async fn call_area(
    base: &str,
    token: &str,
    area_id: &str,
    service: &str,
    data: Value,
) -> Result<(), String> {
    if !allowed("light", service) {
        return Err("Lights don't offer that control.".into());
    }
    let mut body = match data {
        Value::Object(map) => map,
        _ => Default::default(),
    };
    body.insert("area_id".into(), area_id.into());
    let response = client()
        .post(api(base, &format!("services/light/{service}")))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|_| "Couldn't reach Home Assistant. The command wasn't sent.".to_string())?;
    response
        .status()
        .is_success()
        .then_some(())
        .ok_or_else(|| http_error(response.status()))
}

const ADMIN: &str = "Changing this needs a Home Assistant administrator token.";

/// One WebSocket command, such as a registry update; its result.
pub async fn ws_command(base: &str, token: &str, mut command: Value) -> Result<Value, String> {
    let (mut socket, _) = tokio_tungstenite::connect_async(websocket_url(base)?.as_str())
        .await
        .map_err(|_| "Couldn't reach Home Assistant.".to_string())?;
    authenticate(&mut socket, token).await?;
    command["id"] = json!(1);
    socket
        .send(Message::Text(command.to_string().into()))
        .await
        .map_err(|_| "Couldn't reach Home Assistant.".to_string())?;
    loop {
        let response = read_json(&mut socket).await?;
        if response["id"].as_u64() != Some(1) {
            continue;
        }
        let _ = socket.close(None).await;
        if response["success"] == true {
            return Ok(response["result"].clone());
        }
        return Err(match response["error"]["code"].as_str() {
            Some("unauthorized") => ADMIN.into(),
            _ => format!(
                "Home Assistant refused the change: {}",
                response["error"]["message"]
                    .as_str()
                    .unwrap_or("unknown error")
            ),
        });
    }
}

/// The label Home puts on cameras that are plugged in.
pub const PLUGGED_IN: &str = "plugged_in";

/// Mark a device plugged in (or not) with a Home Assistant label, making the
/// label first if it doesn't exist. `labels` are the device's labels now.
pub async fn set_plugged_in(
    base: &str,
    token: &str,
    device_id: &str,
    labels: Vec<String>,
    on: bool,
) -> Result<(), String> {
    if on {
        let list = ws_command(base, token, json!({"type": "config/label_registry/list"})).await?;
        let exists = list
            .as_array()
            .into_iter()
            .flatten()
            .any(|l| l["label_id"] == PLUGGED_IN);
        if !exists {
            ws_command(
                base,
                token,
                json!({"type": "config/label_registry/create", "name": "Plugged in", "icon": "mdi:power-plug"}),
            )
            .await?;
        }
    }
    let mut labels: Vec<String> = labels.into_iter().filter(|l| l != PLUGGED_IN).collect();
    if on {
        labels.push(PLUGGED_IN.into());
    }
    ws_command(
        base,
        token,
        json!({"type": "config/device_registry/update", "device_id": device_id, "labels": labels}),
    )
    .await
    .map(|_| ())
}

/// A live stream of a camera: Home Assistant's HLS playlist URL. The URL
/// carries a short-lived token of its own; don't log or show it.
pub async fn camera_stream(base: &str, token: &str, entity_id: &str) -> Result<String, String> {
    if !entity_id.starts_with("camera.") {
        return Err("That isn't a camera.".into());
    }
    let result = ws_command(
        base,
        token,
        json!({"type": "camera/stream", "entity_id": entity_id, "format": "hls"}),
    )
    .await
    .map_err(|e| {
        if e.contains("refused") {
            "Home Assistant has no live stream for this camera. Some cameras (Eufy, for one) \
             stream only with their RTSP stream setting on."
                .to_string()
        } else {
            e
        }
    })?;
    stream_url(base, &result)
}

/// The full URL from a `camera/stream` result (`{"url": "/api/hls/…"}`).
pub fn stream_url(base: &str, result: &Value) -> Result<String, String> {
    let path = result["url"]
        .as_str()
        .filter(|u| u.starts_with("/api/hls/"))
        .ok_or("Home Assistant didn't return a stream.")?;
    Ok(format!("{}{path}", base.trim_end_matches('/')))
}

/// A request to Home Assistant's config API (`/api/config/...`).
async fn config_request(
    base: &str,
    token: &str,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, String> {
    let mut request = client()
        .request(method, api(base, &format!("config/{path}")))
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|_| "Couldn't reach Home Assistant.".to_string())?;
    match response.status().as_u16() {
        200..=299 => Ok(response.json().await.unwrap_or(Value::Null)),
        401 | 403 => Err(ADMIN.into()),
        _ => Err(http_error(response.status())),
    }
}

/// Step a config or options flow on to its end; an error names what the
/// form complained about.
fn flow_done(step: &Value) -> Result<(), String> {
    match step["type"].as_str() {
        Some("create_entry") => Ok(()),
        Some("abort") => Err(format!(
            "Home Assistant stopped: {}",
            step["reason"].as_str().unwrap_or("unknown reason")
        )),
        _ => Err(
            match step["errors"].as_object().and_then(|e| e.values().next()) {
                Some(error) => format!("Home Assistant didn't accept it: {error}"),
                None => "Home Assistant didn't accept the group.".into(),
            },
        ),
    }
}

/// Create a light group in Home Assistant; its config entry ID.
pub async fn create_light_group(
    base: &str,
    token: &str,
    name: &str,
    members: &[String],
    hide_members: bool,
) -> Result<String, String> {
    let start = config_request(
        base,
        token,
        reqwest::Method::POST,
        "config_entries/flow",
        Some(json!({"handler": "group", "show_advanced_options": false})),
    )
    .await?;
    let flow = start["flow_id"]
        .as_str()
        .ok_or("Home Assistant couldn't start a group.")?
        .to_string();
    let path = format!("config_entries/flow/{flow}");
    let mut step = start;
    if step["type"] == "menu" {
        step = config_request(
            base,
            token,
            reqwest::Method::POST,
            &path,
            Some(json!({"next_step_id": "light"})),
        )
        .await?;
    }
    if step["step_id"] != "light" {
        return Err("Home Assistant offered no light group.".into());
    }
    let done = config_request(
        base,
        token,
        reqwest::Method::POST,
        &path,
        Some(
            json!({"name": name, "entities": members, "hide_members": hide_members, "all": false}),
        ),
    )
    .await?;
    flow_done(&done)?;
    done["result"]["entry_id"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| "The group was made, but Home Assistant didn't say which it is.".into())
}

/// Change which lights a group holds and whether they're hidden.
pub async fn edit_light_group(
    base: &str,
    token: &str,
    entry_id: &str,
    members: &[String],
    hide_members: bool,
) -> Result<(), String> {
    let start = config_request(
        base,
        token,
        reqwest::Method::POST,
        "config_entries/options/flow",
        Some(json!({"handler": entry_id})),
    )
    .await?;
    let flow = start["flow_id"]
        .as_str()
        .ok_or("Home Assistant couldn't edit the group.")?;
    let done = config_request(
        base,
        token,
        reqwest::Method::POST,
        &format!("config_entries/options/flow/{flow}"),
        Some(json!({"entities": members, "hide_members": hide_members, "all": false})),
    )
    .await?;
    flow_done(&done)
}

pub async fn delete_config_entry(base: &str, token: &str, entry_id: &str) -> Result<(), String> {
    config_request(
        base,
        token,
        reqwest::Method::DELETE,
        &format!("config_entries/entry/{entry_id}"),
        None,
    )
    .await
    .map(|_| ())
}

/// The entity a config entry made, once Home Assistant has registered it.
pub async fn entry_entity(
    base: &str,
    token: &str,
    entry_id: &str,
) -> Result<Option<String>, String> {
    let list = ws_command(base, token, json!({"type": "config/entity_registry/list"})).await?;
    Ok(list
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| e["config_entry_id"] == entry_id)
        .and_then(|e| e["entity_id"].as_str())
        .map(str::to_string))
}

/// Integration actions the app may call, for robot maps: clean rooms, mow
/// zones. Anything else is refused before a request is made.
pub fn allowed_integration(domain: &str, service: &str) -> bool {
    matches!(
        (domain, service),
        ("dreame_vacuum", "vacuum_clean_segment") | ("dreame_lawn_mower", "start_zone_mowing")
    )
}

/// Call an integration's own action (see [`allowed_integration`]).
pub async fn call_service(
    base: &str,
    token: &str,
    domain: &str,
    service: &str,
    data: Value,
) -> Result<(), String> {
    if !allowed_integration(domain, service) {
        return Err("Home doesn't send that action.".into());
    }
    let response = client()
        .post(api(base, &format!("services/{domain}/{service}")))
        .bearer_auth(token)
        .json(&data)
        .send()
        .await
        .map_err(|_| "Couldn't reach Home Assistant. The command wasn't sent.".to_string())?;
    response
        .status()
        .is_success()
        .then_some(())
        .ok_or_else(|| http_error(response.status()))
}

/// The mower integration's map data (`/api/dreame_lawn_mower/mowing-map/…`).
pub async fn fetch_mowing_map(base: &str, token: &str, path: &str) -> Result<Value, String> {
    if !path.starts_with("/api/dreame_lawn_mower/mowing-map/") || path.contains("..") {
        return Err("That isn't a mower map.".into());
    }
    let url = format!("{}{path}", base.trim_end_matches('/'));
    let response = client()
        .get(url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|_| "Couldn't reach Home Assistant.".to_string())?;
    if !response.status().is_success() {
        return Err(http_error(response.status()));
    }
    response
        .json()
        .await
        .map_err(|_| "The mower's map data couldn't be read.".into())
}

pub async fn call_robot_service(
    base: &str,
    token: &str,
    entity: &Entity,
    action: RobotAction,
    area_id: Option<&str>,
) -> Result<(), String> {
    if !home_features::robot_actions(entity).contains(&action) {
        return Err("This robot does not offer that control.".into());
    }
    let service = action
        .service(entity.domain())
        .ok_or_else(|| "Unsupported robot control.".to_string())?;
    let body = if action == RobotAction::CleanArea {
        let area = area_id.ok_or_else(|| "Choose an area to clean.".to_string())?;
        json!({"entity_id": entity.entity_id, "cleaning_area_id": [area]})
    } else {
        json!({"entity_id": entity.entity_id})
    };
    let response = client()
        .post(api(
            base,
            &format!("services/{}/{service}", entity.domain()),
        ))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|_| "Could not reach Home Assistant.".to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(http_error(response.status()))
    }
}

pub async fn fetch_entity_picture(
    base: &str,
    token: &str,
    entity: &Entity,
) -> Result<Vec<u8>, String> {
    let endpoint = match entity.domain() {
        "camera" => "camera_proxy",
        "image" => "image_proxy",
        _ => return Err("This entity has no supported image preview.".into()),
    };
    if matches!(entity.state.as_str(), "unavailable" | "unknown" | "off") {
        return Err("This image is unavailable.".into());
    }
    let mut response = client()
        .get(api(base, &format!("{endpoint}/{}", entity.entity_id)))
        .bearer_auth(token)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| "Could not reach Home Assistant.".to_string())?;
    if !response.status().is_success() {
        return Err(format!("Preview unavailable (HTTP {}).", response.status()));
    }
    if !response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|kind| kind.starts_with("image/"))
    {
        return Err("Home Assistant did not return an image.".into());
    }
    const MAX_PREVIEW: usize = 8 * 1024 * 1024;
    if response
        .content_length()
        .is_some_and(|size| size > MAX_PREVIEW as u64)
    {
        return Err("Preview is too large to display.".into());
    }
    let mut data = Vec::new();
    while let Some(part) = response
        .chunk()
        .await
        .map_err(|_| "Could not read the image preview.".to_string())?
    {
        if data.len() + part.len() > MAX_PREVIEW {
            return Err("Preview is too large to display.".into());
        }
        data.extend_from_slice(&part);
    }
    Ok(data)
}

/// Follow Home Assistant's live state until the receiver goes away,
/// reconnecting with a growing delay after each drop.
pub async fn follow(
    base: String,
    token: String,
    sender: Sender<Update>,
    metadata: Arc<HashMap<String, Metadata>>,
) {
    let mut delay = 2;
    loop {
        let started = std::time::Instant::now();
        let error = match websocket_loop(&base, &token, &sender, &metadata).await {
            Ok(()) => return,
            Err(message) => message,
        };
        if sender.send(Update::Offline(error)).await.is_err() {
            return;
        }
        if started.elapsed() > Duration::from_secs(60) {
            delay = 2;
        }
        tokio::time::sleep(Duration::from_secs(delay)).await;
        delay = (delay * 2).min(30);
    }
}

/// Returns `Ok` only when the receiver has gone away.
async fn websocket_loop(
    base: &str,
    token: &str,
    sender: &Sender<Update>,
    metadata: &HashMap<String, Metadata>,
) -> Result<(), String> {
    let (mut socket, _) = tokio_tungstenite::connect_async(websocket_url(base)?.as_str())
        .await
        .map_err(|_| "Home Assistant is offline. Retrying…".to_string())?;
    authenticate(&mut socket, token).await?;
    socket
        .send(Message::Text(
            json!({"id":1,"type":"subscribe_events","event_type":"state_changed"})
                .to_string()
                .into(),
        ))
        .await
        .map_err(|_| "Could not subscribe to Home Assistant updates.".to_string())?;
    if sender.send(Update::Online).await.is_err() {
        return Ok(());
    }
    while let Some(message) = socket.next().await {
        let message = message.map_err(|_| "Connection lost. Retrying…".to_string())?;
        let Ok(text) = message.into_text() else {
            continue;
        };
        let Some(update) = parse_update(&text, metadata) else {
            continue;
        };
        if sender.send(update).await.is_err() {
            return Ok(());
        }
    }
    Err("Connection lost. Retrying…".into())
}

fn parse_update(text: &str, metadata: &HashMap<String, Metadata>) -> Option<Update> {
    let value = serde_json::from_str::<Value>(text).ok()?;
    if value["type"] != "event" {
        return None;
    }
    let data = &value["event"]["data"];
    if data["new_state"].is_null() {
        return data["entity_id"]
            .as_str()
            .map(|id| Update::Removed(id.to_string()));
    }
    let mut entity = serde_json::from_value::<Entity>(data["new_state"].clone()).ok()?;
    if let Some(meta) = metadata.get(&entity.entity_id) {
        entity.apply(meta);
    }
    Some(Update::Entity(entity))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_urls_are_home_assistant_hls() {
        let ok = stream_url(
            "http://ha.local:8123/",
            &json!({"url": "/api/hls/abc/master_playlist.m3u8"}),
        );
        assert_eq!(
            ok.unwrap(),
            "http://ha.local:8123/api/hls/abc/master_playlist.m3u8"
        );
        assert!(stream_url("http://ha", &json!({"url": "https://elsewhere/x.m3u8"})).is_err());
        assert!(stream_url("http://ha", &json!({})).is_err());
    }

    #[test]
    fn only_robot_map_actions_go_to_integrations() {
        assert!(allowed_integration("dreame_vacuum", "vacuum_clean_segment"));
        assert!(allowed_integration(
            "dreame_lawn_mower",
            "start_zone_mowing"
        ));
        assert!(!allowed_integration("dreame_vacuum", "vacuum_delete_map"));
        assert!(!allowed_integration("shell_command", "anything"));
    }

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

    #[test]
    fn live_updates_keep_registry_details() {
        let metadata = HashMap::from([(
            "image.vacuum_map".to_string(),
            Metadata {
                area_name: Some("Kitchen".into()),
                device_name: Some("Robot".into()),
                device_id: Some("device-1".into()),
                entity_category: Some("diagnostic".into()),
                hidden: true,
                ..Default::default()
            },
        )]);
        let event = json!({"type":"event","event":{"data":{
            "entity_id":"image.vacuum_map",
            "new_state":{"entity_id":"image.vacuum_map","state":"2026-10-27","attributes":{}}
        }}})
        .to_string();
        let Some(Update::Entity(entity)) = parse_update(&event, &metadata) else {
            panic!("expected an entity update");
        };
        assert_eq!(entity.device_id.as_deref(), Some("device-1"));
        assert_eq!(entity.entity_category.as_deref(), Some("diagnostic"));
        assert!(entity.hidden);
    }

    #[test]
    fn only_listed_actions_are_allowed() {
        assert!(allowed("light", "turn_on"));
        assert!(allowed("lock", "unlock"));
        assert!(!allowed("lock", "turn_off"));
        assert!(!allowed("shell_command", "turn_on"));
        assert!(allowed("climate", "set_hvac_mode"));
        assert!(allowed("select", "select_option"));
        assert!(allowed("alarm_control_panel", "alarm_disarm"));
        assert!(!allowed("alarm_control_panel", "alarm_trigger"));
        assert!(!allowed("camera", "turn_on"));
    }

    /// Answer one request with `status`, `kind` and `body`; resolve to the request.
    async fn serve_once(
        status: &'static str,
        kind: &'static str,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 4096];
            let read = stream.read(&mut request).await.unwrap();
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&request[..read]).into_owned()
        });
        (format!("http://{address}"), server)
    }

    #[tokio::test]
    async fn fetch_states_accepts_a_mocked_home_assistant_response() {
        let (base, server) = serve_once(
            "200 OK",
            "application/json",
            r#"[{"entity_id":"light.office","state":"on","attributes":{"friendly_name":"Office lights"}}]"#,
        )
        .await;
        let entities = fetch_states(&base, "test-token").await.unwrap();
        let request = server.await.unwrap();
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer test-token")
        );
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].name(), "Office lights");
    }

    #[tokio::test]
    async fn a_rejected_token_is_reported_as_rejected() {
        let (base, server) = serve_once("401 Unauthorized", "text/plain", "").await;
        assert_eq!(check(&base, "bad").await.unwrap_err(), CheckError::Rejected);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn check_reads_the_server_version() {
        let (base, server) = serve_once(
            "200 OK",
            "application/json",
            r#"{"version":"2026.10.1","location_name":"Home Lab"}"#,
        )
        .await;
        let info = check(&base, "test-token").await.unwrap();
        assert_eq!(info.version, "2026.10.1");
        assert_eq!(info.location_name, "Home Lab");
        assert!(server.await.unwrap().starts_with("GET /api/config "));
    }

    #[tokio::test]
    async fn camera_preview_uses_authenticated_proxy() {
        let (base, server) = serve_once("200 OK", "image/png", "PNG!").await;
        let camera: Entity = serde_json::from_value(json!({
            "entity_id":"camera.office","state":"idle","attributes":{}
        }))
        .unwrap();
        let image = fetch_entity_picture(&base, "test-token", &camera)
            .await
            .unwrap();
        let request = server.await.unwrap();
        assert!(request.starts_with("GET /api/camera_proxy/camera.office "));
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer test-token")
        );
        assert_eq!(image, b"PNG!");
    }

    #[tokio::test]
    async fn service_calls_send_the_entity_and_its_data() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 2048];
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                if request.windows(14).any(|part| part == b"brightness_pct") {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]")
                .await
                .unwrap();
            String::from_utf8(request).unwrap()
        });
        call(
            &format!("http://{address}"),
            "test-token",
            "light.office",
            "turn_on",
            json!({"brightness_pct": 40}),
        )
        .await
        .unwrap();
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /api/services/light/turn_on "));
        assert!(request.contains("\"entity_id\":\"light.office\""));
        assert!(request.contains("\"brightness_pct\":40"));
    }

    #[tokio::test]
    async fn vacuum_area_control_uses_mapped_area_id() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0; 2048];
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                if request
                    .windows(b"cleaning_area_id".len())
                    .any(|part| part == b"cleaning_area_id")
                    && request
                        .windows(b"kitchen".len())
                        .any(|part| part == b"kitchen")
                {
                    break;
                }
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("POST /api/services/vacuum/clean_area "));
            assert!(request.contains("\"entity_id\":\"vacuum.test\""));
            assert!(request.contains("\"cleaning_area_id\":[\"kitchen\"]"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]")
                .await
                .unwrap();
        });
        let vacuum: Entity = serde_json::from_value(json!({
            "entity_id":"vacuum.test","state":"docked",
            "attributes":{"supported_features":16384}
        }))
        .unwrap();
        call_robot_service(
            &format!("http://{address}"),
            "test-token",
            &vacuum,
            RobotAction::CleanArea,
            Some("kitchen"),
        )
        .await
        .unwrap();
        server.await.unwrap();
    }
}
