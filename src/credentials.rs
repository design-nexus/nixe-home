//! Secret Service access through `secret-tool`; credential material travels only
//! over stdin/stdout and is never placed in process arguments. Shared by the
//! app and the desktop service so both look up the same item.
use std::io::Write;
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

const TOOL: &str = "secret-tool";
const SERVICE: &str = "nyx-home";
/// Items saved before the suite was renamed from Nixe to Nyx.
const LEGACY_SERVICE: &str = "nixe-home";
const ACCOUNT: &str = "home-assistant";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// No Home Assistant token is saved.
    Missing,
    /// The keyring is locked, missing, or did not answer.
    Unavailable(String),
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => f.write_str("No Home Assistant token is saved."),
            Self::Unavailable(message) => f.write_str(message),
        }
    }
}

/// Load the saved token, moving one saved under the old name to the new one.
/// This can block while the keyring asks to be unlocked; never call it on the
/// GTK thread.
pub fn load(timeout: Duration) -> Result<String, LoadError> {
    load_with(TOOL, timeout)
}

pub fn store(token: &str, timeout: Duration) -> Result<(), String> {
    store_with(TOOL, SERVICE, token, timeout)
}

pub fn delete(timeout: Duration) -> Result<(), String> {
    clear_with(TOOL, SERVICE, timeout)?;
    let _ = clear_with(TOOL, LEGACY_SERVICE, timeout);
    Ok(())
}

fn load_with(tool: &str, timeout: Duration) -> Result<String, LoadError> {
    match lookup_with(tool, SERVICE, timeout) {
        Err(LoadError::Missing) => {
            let token = lookup_with(tool, LEGACY_SERVICE, timeout)?;
            store_with(tool, SERVICE, &token, timeout).map_err(LoadError::Unavailable)?;
            let _ = clear_with(tool, LEGACY_SERVICE, timeout);
            Ok(token)
        }
        result => result,
    }
}

fn command(tool: &str, verb: &str, service: &str) -> Command {
    let mut command = Command::new(tool);
    command.arg(verb);
    if verb == "store" {
        command.arg("--label=Home Assistant (Nyx Home)");
    }
    command.args(["service", service, "account", ACCOUNT]);
    command
}

fn unavailable() -> String {
    "Secret Service is unavailable. Unlock your keyring and try again.".into()
}

fn lookup_with(tool: &str, service: &str, timeout: Duration) -> Result<String, LoadError> {
    let child = command(tool, "lookup", service)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| LoadError::Unavailable(unavailable()))?;
    let output = wait(child, timeout).map_err(LoadError::Unavailable)?;
    let token = String::from_utf8(output.stdout)
        .map_err(|_| LoadError::Unavailable("The saved token could not be read.".into()))?
        .trim_end()
        .to_string();
    if output.status.success() && !token.is_empty() {
        Ok(token)
    } else if output.stderr.iter().all(u8::is_ascii_whitespace) {
        Err(LoadError::Missing)
    } else {
        Err(LoadError::Unavailable(unavailable()))
    }
}

fn store_with(tool: &str, service: &str, token: &str, timeout: Duration) -> Result<(), String> {
    let mut child = command(tool, "store", service)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| unavailable())?;
    let mut input = child.stdin.take().ok_or_else(unavailable)?;
    let written = input.write_all(token.as_bytes());
    drop(input);
    let output = wait(child, timeout)?;
    written.map_err(|_| "Could not save the Home Assistant token.".to_string())?;
    output.status.success().then_some(()).ok_or_else(|| {
        "Secret Service could not save the token. Check that your keyring is unlocked.".into()
    })
}

fn clear_with(tool: &str, service: &str, timeout: Duration) -> Result<(), String> {
    let child = command(tool, "clear", service)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| unavailable())?;
    let output = wait(child, timeout)?;
    output
        .status
        .success()
        .then_some(())
        .ok_or_else(|| "The saved token could not be removed.".into())
}

fn wait(mut child: Child, limit: Duration) -> Result<Output, String> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().map_err(|_| unavailable()),
            Ok(None) if started.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("The keyring did not respond. Unlock it, then try again.".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A stand-in for `secret-tool` that keeps items as files.
    fn fake_tool() -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("nyx-home-secrets-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("store")).unwrap();
        let script = dir.join("secret-tool");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nverb=$1; shift; key=\nfor a in \"$@\"; do case $a in --label=*) ;; *) key=\"$key-$a\";; esac; done\nf=\"{}/store/$key\"\ncase $verb in store) cat > \"$f\";; lookup) [ -f \"$f\" ] && cat \"$f\" || exit 1;; clear) rm -f \"$f\";; esac\n",
                dir.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, script.display().to_string())
    }

    // One test, so no other test thread holds a script open for writing while
    // this one is executed ("text file busy").
    #[test]
    fn tokens_saved_under_the_old_name_move_to_the_new_one() {
        let (dir, tool) = fake_tool();
        let limit = Duration::from_secs(5);
        assert_eq!(load_with(&tool, limit), Err(LoadError::Missing));
        store_with(&tool, LEGACY_SERVICE, "legacy-token", limit).unwrap();
        assert_eq!(load_with(&tool, limit).unwrap(), "legacy-token");
        assert_eq!(
            lookup_with(&tool, LEGACY_SERVICE, limit),
            Err(LoadError::Missing)
        );
        assert_eq!(lookup_with(&tool, SERVICE, limit).unwrap(), "legacy-token");

        // A current token wins over one left under the old name.
        store_with(&tool, LEGACY_SERVICE, "old", limit).unwrap();
        store_with(&tool, SERVICE, "new", limit).unwrap();
        assert_eq!(load_with(&tool, limit).unwrap(), "new");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
