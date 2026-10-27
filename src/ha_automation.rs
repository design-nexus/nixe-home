use nyx_home::automation::Workflow;

pub async fn save(base: &str, token: &str, workflow: &Workflow) -> Result<(), String> {
    let config = workflow.to_home_assistant()?;
    let url = format!(
        "{}/api/config/automation/config/{}",
        base.trim_end_matches('/'),
        workflow.id
    );
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()
        .map_err(|_| "Could not initialize Home Assistant connection".to_string())?
        .post(url)
        .bearer_auth(token)
        .json(&config)
        .send()
        .await
        .map_err(|_| "Could not reach Home Assistant".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Home Assistant rejected the automation (HTTP {}). Administrator access and editable automations are required.",
            response.status()
        ));
    }
    Ok(())
}

pub async fn control(
    base: &str,
    token: &str,
    workflow_id: &str,
    service: &str,
) -> Result<(), String> {
    if !matches!(service, "turn_on" | "turn_off" | "trigger") {
        return Err("Unsupported automation action".into());
    }
    let states = crate::ha::fetch_states(base, token).await?;
    let entity=states.into_iter().find(|state| state.domain()=="automation" && state.attributes["id"].as_str()==Some(workflow_id))
        .ok_or("Home Assistant has not exposed this automation yet. Wait for its reload and try again.")?;
    let url = format!(
        "{}/api/services/automation/{service}",
        base.trim_end_matches('/')
    );
    let mut body = serde_json::json!({"entity_id":entity.entity_id});
    if service == "trigger" {
        body["skip_condition"] = serde_json::Value::Bool(false);
    }
    let response = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(12))
        .build()
        .map_err(|_| "Could not initialize Home Assistant connection".to_string())?
        .post(url)
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .map_err(|_| "Could not reach Home Assistant".to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Home Assistant rejected the action (HTTP {})",
            response.status()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nyx_home::automation::{Edge, Node, NodeKind, Owner};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn posts_supported_automation_to_config_api() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let n = stream.read(&mut buffer).await.unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|v| v.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let text = String::from_utf8(request).unwrap();
            assert!(text.starts_with("POST /api/config/automation/config/test-home HTTP/1.1"));
            assert!(
                text.contains("authorization: Bearer test-token")
                    || text.contains("Authorization: Bearer test-token")
            );
            assert!(text.contains("\"action\":\"light.turn_on\""));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 15\r\n\r\n{\"result\":\"ok\"}")
                .await
                .unwrap();
        });
        let workflow = Workflow {
            schema_version: 1,
            id: "test-home".into(),
            name: "Test".into(),
            enabled: true,
            owner: Owner::HomeAssistant,
            nodes: vec![
                Node {
                    id: "when".into(),
                    kind: NodeKind::EntityState {
                        entity_id: "binary_sensor.motion".into(),
                        state: "on".into(),
                    },
                },
                Node {
                    id: "then".into(),
                    kind: NodeKind::SetEntity {
                        entity_id: "light.office".into(),
                        turn_on: true,
                    },
                },
            ],
            edges: vec![Edge {
                from: "when".into(),
                to: "then".into(),
                output: None,
            }],
            positions: Default::default(),
            policies: Default::default(),
            groups: Default::default(),
        };
        save(&format!("http://{address}"), "test-token", &workflow)
            .await
            .unwrap();
        server.await.unwrap();
    }
}
