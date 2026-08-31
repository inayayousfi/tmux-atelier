use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::config::Config;
use crate::process;
use crate::workspace::{self, Workspace};
use crate::{Result, err};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub active: String,
    pub workspaces: Vec<SnapshotWorkspace>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotWorkspace {
    pub name: String,
    pub active_window: u32,
    pub windows: Vec<SnapshotWindow>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotWindow {
    pub index: u32,
    pub name: String,
    pub automatic_rename: bool,
    pub layout: String,
    pub active_pane: usize,
    pub paths: Vec<String>,
}

impl Snapshot {
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::new();
        field(&mut output, "tmux-atelier-restore");
        field(&mut output, "2");
        field(&mut output, "active");
        field(&mut output, &self.active);
        for workspace in &self.workspaces {
            field(&mut output, "workspace");
            field(&mut output, &workspace.name);
            field(&mut output, &workspace.active_window.to_string());
            for window in &workspace.windows {
                field(&mut output, "window");
                field(&mut output, &window.index.to_string());
                field(&mut output, &window.name);
                field(
                    &mut output,
                    if window.automatic_rename { "on" } else { "off" },
                );
                field(&mut output, &window.layout);
                field(&mut output, &window.paths.len().to_string());
                field(&mut output, &window.active_pane.to_string());
                for path in &window.paths {
                    field(&mut output, "pane");
                    field(&mut output, path);
                }
                field(&mut output, "end-window");
            }
            field(&mut output, "end-workspace");
        }
        field(&mut output, "end");
        output
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.last() != Some(&0) {
            return Err(err("invalid restore snapshot"));
        }
        let mut tokens = bytes[..bytes.len() - 1]
            .split(|byte| *byte == 0)
            .map(|token| String::from_utf8(token.to_vec()))
            .collect::<std::result::Result<Vec<_>, _>>()?
            .into_iter();
        expect(&mut tokens, "tmux-atelier-restore")?;
        expect(&mut tokens, "2")?;
        expect(&mut tokens, "active")?;
        let active = next(&mut tokens)?;
        if !active.is_empty() {
            workspace::validate_name(&active).map_err(|_| err("invalid restore snapshot"))?;
        }
        let mut workspaces = Vec::new();
        let mut names = HashSet::new();
        loop {
            let token = next(&mut tokens)?;
            if token == "end" {
                if tokens.next().is_some() || !names.contains(&active) {
                    return Err(err("invalid restore snapshot"));
                }
                break;
            }
            if token != "workspace" {
                return Err(err("invalid restore snapshot"));
            }
            let name = next(&mut tokens)?;
            workspace::validate_name(&name).map_err(|_| err("invalid restore snapshot"))?;
            if !names.insert(name.clone()) {
                return Err(err("invalid restore snapshot"));
            }
            let active_window = number(&mut tokens)?;
            let mut windows = Vec::new();
            let mut indexes = HashSet::new();
            loop {
                let token = next(&mut tokens)?;
                if token == "end-workspace" {
                    break;
                }
                if token != "window" {
                    return Err(err("invalid restore snapshot"));
                }
                let index = number(&mut tokens)?;
                if !indexes.insert(index) {
                    return Err(err("invalid restore snapshot"));
                }
                let window_name = next(&mut tokens)?;
                reject_newline(&window_name)?;
                let automatic_rename = match next(&mut tokens)?.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err(err("invalid restore snapshot")),
                };
                let layout = next(&mut tokens)?;
                if layout.is_empty() {
                    return Err(err("invalid restore snapshot"));
                }
                reject_newline(&layout)?;
                let pane_count = positive_number(&mut tokens)? as usize;
                let active_pane = number(&mut tokens)? as usize;
                if active_pane >= pane_count {
                    return Err(err("invalid restore snapshot"));
                }
                let mut paths = Vec::with_capacity(pane_count);
                for _ in 0..pane_count {
                    expect(&mut tokens, "pane")?;
                    paths.push(next(&mut tokens)?);
                }
                expect(&mut tokens, "end-window")?;
                windows.push(SnapshotWindow {
                    index,
                    name: window_name,
                    automatic_rename,
                    layout,
                    active_pane,
                    paths,
                });
            }
            if windows.is_empty() || !indexes.contains(&active_window) {
                return Err(err("invalid restore snapshot"));
            }
            workspaces.push(SnapshotWorkspace {
                name,
                active_window,
                windows,
            });
        }
        Ok(Self { active, workspaces })
    }

    pub fn counts(&self) -> (usize, usize, usize) {
        let windows = self
            .workspaces
            .iter()
            .map(|workspace| workspace.windows.len())
            .sum();
        let panes = self
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.windows)
            .map(|window| window.paths.len())
            .sum();
        (self.workspaces.len(), windows, panes)
    }
}

fn field(output: &mut Vec<u8>, value: &str) {
    output.extend_from_slice(value.as_bytes());
    output.push(0);
}

fn next(tokens: &mut impl Iterator<Item = String>) -> Result<String> {
    tokens.next().ok_or_else(|| err("invalid restore snapshot"))
}

fn expect(tokens: &mut impl Iterator<Item = String>, expected: &str) -> Result<()> {
    (next(tokens)? == expected)
        .then_some(())
        .ok_or_else(|| err("invalid restore snapshot"))
}

fn number(tokens: &mut impl Iterator<Item = String>) -> Result<u32> {
    let value = next(tokens)?;
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(err("invalid restore snapshot"));
    }
    value.parse().map_err(|_| err("invalid restore snapshot"))
}

fn positive_number(tokens: &mut impl Iterator<Item = String>) -> Result<u32> {
    let value = number(tokens)?;
    (value > 0)
        .then_some(value)
        .ok_or_else(|| err("invalid restore snapshot"))
}

fn reject_newline(value: &str) -> Result<()> {
    (!value.contains('\n'))
        .then_some(())
        .ok_or_else(|| err("invalid restore snapshot"))
}

pub fn read(config: &Config) -> Result<Snapshot> {
    Snapshot::decode(&fs::read(&config.restore_file)?)
}

pub fn lock<T>(config: &Config, path: &Path, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    config.secure_dir(&config.state_root)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(path)?;
    let mut acquired = false;
    for _ in 0..500 {
        match file.try_lock() {
            Ok(()) => {
                acquired = true;
                break;
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::Error(error)) => return Err(error.into()),
        }
    }
    if !acquired {
        return Err(err(if path == config.snapshot_lock {
            "could not acquire snapshot lock"
        } else {
            "could not acquire session adoption lock"
        }));
    }
    let result = operation();
    let unlock = file.unlock();
    match (result, unlock) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error.into()),
        (Ok(value), Ok(())) => Ok(value),
    }
}

pub fn write(config: &Config, exclude_session: &str, exclude_window: &str) -> Result<()> {
    if process::tmux_quiet(
        config,
        &["show-options", "-gqv", "@atelier_restore_pending"],
    )
    .as_deref()
        == Some("1")
    {
        return Ok(());
    }
    let mut sessions = Vec::new();
    let mut active = String::new();
    let mut active_activity: i64 = -1;
    for name in workspace::session_names(config) {
        if workspace::session_option(config, &name, "@atelier_managed") != "1"
            || !config.workspaces.join(&name).is_file()
            || name == exclude_session
        {
            continue;
        }
        let activity: i64 = process::tmux_output(
            config,
            &[
                "display-message",
                "-p",
                "-t",
                &format!("={name}:"),
                "#{session_activity}",
            ],
        )?
        .parse()?;
        if activity > active_activity {
            active = name.clone();
            active_activity = activity;
        }
        sessions.push(name);
    }
    if sessions.is_empty() {
        match fs::remove_file(&config.restore_file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        return Ok(());
    }
    let mut saved = Vec::new();
    for name in sessions {
        let mut active_window: u32 = process::tmux_output(
            config,
            &[
                "display-message",
                "-p",
                "-t",
                &format!("={name}:"),
                "#{window_index}",
            ],
        )?
        .parse()?;
        let windows = lines(process::tmux_output(
            config,
            &[
                "list-windows",
                "-t",
                &format!("={name}:"),
                "-F",
                "#{window_id}",
            ],
        )?);
        let windows: Vec<_> = windows
            .into_iter()
            .filter(|window| window != exclude_window)
            .collect();
        if windows.is_empty() {
            continue;
        }
        if !exclude_window.is_empty() {
            let current_id = process::tmux_output(
                config,
                &[
                    "display-message",
                    "-p",
                    "-t",
                    &format!("={name}:{active_window}"),
                    "#{window_id}",
                ],
            )?;
            if current_id == exclude_window {
                active_window = process::tmux_output(
                    config,
                    &[
                        "display-message",
                        "-p",
                        "-t",
                        &windows[0],
                        "#{window_index}",
                    ],
                )?
                .parse()?;
            }
        }
        let destination = workspace::session_option(config, &name, "@atelier_destination");
        let mut saved_windows = Vec::new();
        for window in windows {
            let panes = lines(process::tmux_output(
                config,
                &["list-panes", "-t", &window, "-F", "#{pane_id}"],
            )?);
            let mut active_pane = 0;
            let mut paths = Vec::new();
            for (index, pane) in panes.iter().enumerate() {
                if process::tmux_output(
                    config,
                    &["display-message", "-p", "-t", pane, "#{pane_active}"],
                )? == "1"
                {
                    active_pane = index;
                }
                paths.push(if destination == "local" {
                    process::tmux_output(
                        config,
                        &["display-message", "-p", "-t", pane, "#{pane_current_path}"],
                    )?
                } else {
                    String::new()
                });
            }
            saved_windows.push(SnapshotWindow {
                index: process::tmux_output(
                    config,
                    &["display-message", "-p", "-t", &window, "#{window_index}"],
                )?
                .parse()?,
                name: process::tmux_output(
                    config,
                    &["display-message", "-p", "-t", &window, "#{window_name}"],
                )?,
                automatic_rename: process::tmux_output(
                    config,
                    &["show-options", "-wAqv", "-t", &window, "automatic-rename"],
                )? == "on",
                layout: process::tmux_output(
                    config,
                    &["display-message", "-p", "-t", &window, "#{window_layout}"],
                )?,
                active_pane,
                paths,
            });
        }
        saved.push(SnapshotWorkspace {
            name,
            active_window,
            windows: saved_windows,
        });
    }
    let snapshot = Snapshot {
        active,
        workspaces: saved,
    };
    config.secure_dir(&config.state_root)?;
    let temporary = config
        .state_root
        .join(format!(".restore.{}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&snapshot.encode())?;
        file.sync_all()?;
        fs::rename(&temporary, &config.restore_file)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn lines(value: String) -> Vec<String> {
    value
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn restore(config: &Config, snapshot: &Snapshot) -> Result<Vec<String>> {
    let mut created = Vec::new();
    for saved in &snapshot.workspaces {
        if workspace::session_exists(config, &saved.name)
            || !config.workspaces.join(&saved.name).is_file()
        {
            continue;
        }
        let definition = workspace::read(config, &saved.name)?;
        let result = restore_workspace(config, &definition, saved);
        if let Err(error) = result {
            if workspace::session_exists(config, &saved.name) {
                let _ = process::tmux(config, &["kill-session", "-t", &format!("={}", saved.name)]);
            }
            for name in &created {
                let _ = process::tmux(config, &["kill-session", "-t", &format!("={name}")]);
            }
            return Err(error);
        }
        created.push(saved.name.clone());
    }
    Ok(created)
}

fn restore_workspace(
    config: &Config,
    definition: &Workspace,
    saved: &SnapshotWorkspace,
) -> Result<()> {
    let mut restored_active = String::new();
    for (ordinal, saved_window) in saved.windows.iter().enumerate() {
        let first_path = saved_window
            .paths
            .first()
            .map(String::as_str)
            .unwrap_or(&definition.path);
        let window = if ordinal == 0 {
            workspace::create_session(config, definition, first_path)?;
            process::tmux_output(
                config,
                &[
                    "display-message",
                    "-p",
                    "-t",
                    &format!("={}:", definition.name),
                    "#{window_id}",
                ],
            )?
        } else if definition.destination == "local" {
            let initial = if Path::new(first_path).is_dir() {
                first_path
            } else {
                &definition.path
            };
            process::tmux_output(
                config,
                &[
                    "new-window",
                    "-a",
                    "-d",
                    "-P",
                    "-F",
                    "#{window_id}",
                    "-t",
                    &format!("={}:{{end}}", definition.name),
                    "-c",
                    initial,
                ],
            )?
        } else {
            let command = process::remote_shell_command(
                config,
                &definition.destination,
                &definition.path,
                &definition.shell,
            )?;
            process::tmux_output(
                config,
                &[
                    "new-window",
                    "-a",
                    "-d",
                    "-P",
                    "-F",
                    "#{window_id}",
                    "-t",
                    &format!("={}:{{end}}", definition.name),
                    &command,
                ],
            )?
        };
        for path in saved_window.paths.iter().skip(1) {
            if definition.destination == "local" {
                let initial = if Path::new(path).is_dir() {
                    path
                } else {
                    &definition.path
                };
                process::tmux(
                    config,
                    &["split-window", "-d", "-t", &window, "-c", initial],
                )?;
            } else {
                let command = process::remote_shell_command(
                    config,
                    &definition.destination,
                    &definition.path,
                    &definition.shell,
                )?;
                process::tmux(config, &["split-window", "-d", "-t", &window, &command])?;
            }
        }
        process::tmux(
            config,
            &["select-layout", "-t", &window, &saved_window.layout],
        )?;
        process::tmux(
            config,
            &["rename-window", "-t", &window, &saved_window.name],
        )?;
        process::tmux(
            config,
            &[
                "set-option",
                "-wq",
                "-t",
                &window,
                "automatic-rename",
                if saved_window.automatic_rename {
                    "on"
                } else {
                    "off"
                },
            ],
        )?;
        let panes = lines(process::tmux_output(
            config,
            &["list-panes", "-t", &window, "-F", "#{pane_id}"],
        )?);
        if definition.destination == "local" {
            for (pane, path) in panes.iter().zip(&saved_window.paths) {
                let initial = if Path::new(path).is_dir() {
                    path
                } else {
                    &definition.path
                };
                process::tmux(config, &["respawn-pane", "-k", "-t", pane, "-c", initial])?;
            }
        }
        process::tmux(
            config,
            &["select-pane", "-t", &panes[saved_window.active_pane]],
        )?;
        if saved_window.index == saved.active_window {
            restored_active = window;
        }
    }
    process::tmux(config, &["select-window", "-t", &restored_active])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_round_trip() {
        let snapshot = Snapshot {
            active: "main".into(),
            workspaces: vec![SnapshotWorkspace {
                name: "main".into(),
                active_window: 1,
                windows: vec![SnapshotWindow {
                    index: 1,
                    name: "source".into(),
                    automatic_rename: false,
                    layout: "abcd,80x24,0,0,1".into(),
                    active_pane: 0,
                    paths: vec!["/tmp/a path".into()],
                }],
            }],
        };
        assert_eq!(Snapshot::decode(&snapshot.encode()).unwrap(), snapshot);
    }

    #[test]
    fn snapshot_rejects_trailing_and_duplicate_data() {
        let mut bytes = b"tmux-atelier-restore\x002\0active\0missing\0end\0".to_vec();
        assert!(Snapshot::decode(&bytes).is_err());
        bytes.push(0);
        assert!(Snapshot::decode(&bytes).is_err());
    }
}
