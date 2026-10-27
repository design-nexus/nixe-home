//! Secret Service access through `secret-tool`; credential material travels only
//! over stdin/stdout and is never placed in process arguments.
use std::io::Write;
use std::process::{Command, Stdio};

const SERVICE: &str = "nyx-home";
const ACCOUNT: &str = "home-assistant";

pub fn store(token: &str) -> Result<(), String> {
    let mut child = Command::new("secret-tool")
        .args([
            "store",
            "--label=Home Assistant (Nyx Home)",
            "service",
            SERVICE,
            "account",
            ACCOUNT,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            "Secret Service is unavailable. Unlock your keyring and try again.".to_string()
        })?;
    child
        .stdin
        .take()
        .ok_or_else(|| "Could not access Secret Service input.".to_string())?
        .write_all(token.as_bytes())
        .map_err(|_| "Could not save the Home Assistant credential.".to_string())?;
    let status = child
        .wait()
        .map_err(|_| "Could not save the Home Assistant credential.".to_string())?;
    status.success().then_some(()).ok_or_else(|| {
        "Secret Service could not save the credential. Check that your keyring is unlocked."
            .to_string()
    })
}

pub fn load() -> Result<String, String> {
    let output = Command::new("secret-tool")
        .args(["lookup", "service", SERVICE, "account", ACCOUNT])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|_| {
            "Secret Service is unavailable. Unlock your keyring and reconnect.".to_string()
        })?;
    if !output.status.success() {
        return Err("No Home Assistant credential is saved.".into());
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim_end().to_string())
        .map_err(|_| "The saved credential could not be read.".into())
}

pub fn delete() -> Result<(), String> {
    let status = Command::new("secret-tool")
        .args(["clear", "service", SERVICE, "account", ACCOUNT])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|_| "Secret Service is unavailable.".to_string())?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "The saved credential could not be removed.".into())
}
