//! Home Assistant transport for desktop-owned hybrid workflows.
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::process::Command;
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone)]
pub struct HybridClient {
    base: String,
    token: String,
    http: reqwest::Client,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StateEvent {
    pub id: String,
    pub entity_id: String,
    pub state: String,
}

#[derive(Deserialize)]
struct ConnectionPrefs {
    home_url: String,
}

impl HybridClient {
    pub async fn load() -> Result<Self, String> {
        let settings = std::fs::read_to_string(nixe_core::paths::prefs_file())
            .map_err(|_| "Connect Home Assistant in Home first".to_string())?;
        let prefs: ConnectionPrefs =
            toml::from_str(&settings).map_err(|_| "Home settings could not be read".to_string())?;
        let url = url::Url::parse(&prefs.home_url)
            .map_err(|_| "Invalid Home Assistant URL".to_string())?;
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Invalid Home Assistant URL".into());
        }
        let output = tokio::time::timeout(
            Duration::from_secs(6),
            Command::new("secret-tool")
                .args([
                    "lookup",
                    "service",
                    "nixe-home",
                    "account",
                    "home-assistant",
                ])
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output(),
        )
        .await
        .map_err(|_| "Secret Service timed out".to_string())?
        .map_err(|_| "Secret Service is unavailable".to_string())?;
        if !output.status.success() {
            return Err("Home Assistant credential is unavailable".into());
        }
        let token = String::from_utf8(output.stdout)
            .map_err(|_| "Invalid Home Assistant credential".to_string())?
            .trim_end()
            .to_owned();
        if token.is_empty() {
            return Err("Home Assistant credential is unavailable".into());
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(12))
            .build()
            .map_err(|_| "HTTP client unavailable".to_string())?;
        Ok(Self {
            base: prefs.home_url.trim_end_matches('/').into(),
            token,
            http,
        })
    }

    pub async fn state(&self, entity_id: &str) -> Result<String, String> {
        let response = self
            .http
            .get(format!("{}/api/states/{entity_id}", self.base))
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|_| "Home Assistant is offline".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "Home Assistant returned HTTP {}",
                response.status()
            ));
        }
        let state: Value = response
            .json()
            .await
            .map_err(|_| "Invalid Home Assistant response".to_string())?;
        state["state"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "Home Assistant state is unavailable".into())
    }

    pub async fn call(&self, domain: &str, service: &str, entity_id: &str) -> Result<(), String> {
        if !matches!(
            (domain, service),
            (
                "light" | "switch" | "fan" | "input_boolean",
                "turn_on" | "turn_off"
            ) | ("scene", "turn_on")
        ) {
            return Err("Unsupported Home Assistant action".into());
        }
        let response = self
            .http
            .post(format!("{}/api/services/{domain}/{service}", self.base))
            .bearer_auth(&self.token)
            .json(&json!({"entity_id":entity_id}))
            .send()
            .await
            .map_err(|_| "Home Assistant is offline; action was not confirmed".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "Home Assistant returned HTTP {}",
                response.status()
            ));
        }
        Ok(())
    }

    pub async fn listen_events(
        &self,
        online: Arc<AtomicBool>,
        mut on_event: impl FnMut(StateEvent),
    ) -> Result<(), String> {
        let mut url =
            url::Url::parse(&self.base).map_err(|_| "Invalid Home Assistant URL".to_string())?;
        url.set_scheme(if url.scheme() == "https" { "wss" } else { "ws" })
            .map_err(|_| "Invalid Home Assistant URL".to_string())?;
        url.set_path("/api/websocket");
        let (mut socket, _) = tokio::time::timeout(
            Duration::from_secs(12),
            tokio_tungstenite::connect_async(url.as_str()),
        )
        .await
        .map_err(|_| "Home Assistant connection timed out".to_string())?
        .map_err(|_| "Home Assistant is offline".to_string())?;
        let hello = next_json(&mut socket).await?;
        if hello["type"] != "auth_required" {
            return Err("Unexpected Home Assistant handshake".into());
        }
        socket
            .send(Message::Text(
                json!({"type":"auth","access_token":self.token})
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(|_| "Home Assistant authentication failed".to_string())?;
        if next_json(&mut socket).await?["type"] != "auth_ok" {
            return Err("Home Assistant authentication failed".into());
        }
        socket
            .send(Message::Text(
                json!({"id":1,"type":"subscribe_events","event_type":"state_changed"})
                    .to_string()
                    .into(),
            ))
            .await
            .map_err(|_| "Home Assistant subscription failed".to_string())?;
        let result = next_json(&mut socket).await?;
        if result["type"] != "result" || result["id"] != 1 || result["success"] != true {
            return Err("Home Assistant subscription rejected".into());
        }
        online.store(true, Ordering::Relaxed);
        loop {
            let message = tokio::select! {
                message = socket.next() => message,
                _ = tokio::time::sleep(Duration::from_secs(10)) => {
                    match Self::load().await {
                        Ok(current) if current.base == self.base && current.token == self.token => continue,
                        _ => return Err("Home Assistant connection changed".into()),
                    }
                }
            };
            let Some(message) = message else {
                break;
            };
            let message = message.map_err(|_| "Home Assistant connection lost".to_string())?;
            let Ok(text) = message.into_text() else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            if let Some(event) = parse_state_event(&value) {
                on_event(event);
            }
        }
        Err("Home Assistant connection closed".into())
    }
}

async fn next_json<S>(socket: &mut tokio_tungstenite::WebSocketStream<S>) -> Result<Value, String>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let next = tokio::time::timeout(Duration::from_secs(12), socket.next())
        .await
        .map_err(|_| "Home Assistant timed out".to_string())?;
    let text = next
        .and_then(Result::ok)
        .and_then(|m| m.into_text().ok())
        .ok_or_else(|| "Home Assistant connection closed".to_string())?;
    serde_json::from_str(&text).map_err(|_| "Invalid Home Assistant response".into())
}

pub fn parse_state_event(value: &Value) -> Option<StateEvent> {
    if value["type"] != "event" {
        return None;
    }
    let event = &value["event"];
    if event["event_type"] != "state_changed" {
        return None;
    }
    let data = &event["data"];
    let entity_id = data["entity_id"].as_str()?;
    let state = data["new_state"]["state"].as_str()?;
    let old_state = data["old_state"]["state"].as_str()?;
    if state == old_state {
        return None;
    }
    let fired = event["time_fired"].as_str()?;
    let context = event["context"]["id"].as_str().unwrap_or("");
    if entity_id.len() > 128 || state.len() > 128 || fired.len() > 64 || context.len() > 128 {
        return None;
    }
    Some(StateEvent {
        id: format!("{entity_id}|{fired}|{context}|{state}"),
        entity_id: entity_id.into(),
        state: state.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[test]
    fn state_events_have_stable_ids_and_ignore_attribute_only_updates() {
        let mut event = json!({"type":"event","event":{"event_type":"state_changed","time_fired":"2026-10-22T12:00:00Z","context":{"id":"context-1"},"data":{"entity_id":"binary_sensor.motion","old_state":{"state":"off"},"new_state":{"state":"on"}}}});
        let parsed = parse_state_event(&event).unwrap();
        assert_eq!(parsed.entity_id, "binary_sensor.motion");
        assert_eq!(parsed, parse_state_event(&event).unwrap());
        event["event"]["data"]["old_state"]["state"] = json!("on");
        assert!(parse_state_event(&event).is_none());
    }

    #[tokio::test]
    async fn posts_only_typed_home_action() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0u8; 4096];
            let mut bytes = Vec::new();
            loop {
                let read = stream.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..read]);
                if bytes
                    .windows(b"light.office".len())
                    .any(|part| part == b"light.office")
                {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&bytes);
            assert!(request.starts_with("POST /api/services/light/turn_on HTTP/1.1"));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("authorization: bearer test-token")
            );
            assert!(request.contains("light.office"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n[]")
                .await
                .unwrap();
        });
        let client = HybridClient {
            base: format!("http://{address}"),
            token: "test-token".into(),
            http: reqwest::Client::new(),
        };
        client
            .call("light", "turn_on", "light.office")
            .await
            .unwrap();
        assert!(client.call("lock", "unlock", "lock.front").await.is_err());
        server.await.unwrap();
    }
}
