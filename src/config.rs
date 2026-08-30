use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Result;

#[derive(Clone, Debug)]
pub struct Config {
    pub tmux: String,
    pub state_root: PathBuf,
    pub workspaces: PathBuf,
    pub debug_log: PathBuf,
    pub restore_file: PathBuf,
    pub snapshot_lock: PathBuf,
    pub adoption_lock: PathBuf,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let tmux = env::var("TMUX_ATELIER_TMUX").unwrap_or_else(|_| "tmux".into());
        let state_root = if let Some(path) = env::var_os("TMUX_ATELIER_STATE_DIR") {
            PathBuf::from(path)
        } else if let Some(path) = env::var_os("XDG_STATE_HOME") {
            PathBuf::from(path).join("tmux-atelier")
        } else {
            PathBuf::from(env::var_os("HOME").ok_or("HOME is not configured")?)
                .join(".local/state/tmux-atelier")
        };
        let debug_log = env::var_os("TMUX_ATELIER_DEBUG_LOG")
            .map(PathBuf::from)
            .unwrap_or_else(|| state_root.join("debug.log"));
        Ok(Self {
            tmux,
            workspaces: state_root.join("workspaces"),
            debug_log,
            restore_file: state_root.join("restore.snapshot"),
            snapshot_lock: state_root.join(".snapshot.lock"),
            adoption_lock: state_root.join(".adoption.lock"),
            state_root,
        })
    }

    pub fn secure_dir(&self, path: &std::path::Path) -> Result<()> {
        fs::create_dir_all(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    pub fn debug(&self, message: &str) -> Result<()> {
        self.secure_dir(&self.state_root)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.debug_log)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        let seconds = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        writeln!(
            file,
            "{seconds} pid={} pane={} client={} {message}",
            std::process::id(),
            shell_debug(&env::var("TMUX_PANE").unwrap_or_default()),
            shell_debug(&env::var("TMUX_ATELIER_CLIENT").unwrap_or_default())
        )?;
        Ok(())
    }
}

pub fn shell_debug(value: &str) -> String {
    if value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_+-./:@".contains(&b))
    {
        value.to_owned()
    } else {
        quote_sh(value)
    }
}

pub fn quote_sh(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn quote_powershell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quotes_apostrophes() {
        assert_eq!(quote_sh("it's here"), "'it'\\''s here'");
        assert_eq!(quote_powershell("it's"), "'it''s'");
    }
}
