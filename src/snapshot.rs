use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::process;
use crate::process_state::{
    self, ObservedForeground, ProcessInspector, RestartPolicy, SavedProcess, SavedShell,
};
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
    pub panes: Vec<SnapshotPane>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotPane {
    pub path: String,
    pub policy: RestartPolicy,
    pub shell: Option<SavedShell>,
    pub process: Option<SavedProcess>,
}

#[derive(Clone, Debug)]
pub struct WorkspaceTopology {
    pub name: String,
    pub active_window: u32,
    pub windows: Vec<WindowTopology>,
}

#[derive(Clone, Debug)]
pub struct WindowTopology {
    pub index: u32,
    pub name: String,
    pub automatic_rename: bool,
    pub layout: String,
    pub active_pane: usize,
    pub panes: Vec<PaneTopology>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaneTopology {
    pub path: String,
}

impl SnapshotWorkspace {
    pub fn topology(&self) -> WorkspaceTopology {
        WorkspaceTopology {
            name: self.name.clone(),
            active_window: self.active_window,
            windows: self
                .windows
                .iter()
                .map(|window| WindowTopology {
                    index: window.index,
                    name: window.name.clone(),
                    automatic_rename: window.automatic_rename,
                    layout: window.layout.clone(),
                    active_pane: window.active_pane,
                    panes: window
                        .panes
                        .iter()
                        .map(|pane| PaneTopology {
                            path: pane.path.clone(),
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessChange {
    pub key: String,
    pub workspace: String,
    pub window: u32,
    pub pane: usize,
    pub program: String,
    pub current: Option<String>,
    pub observed: ObservedForeground,
}

impl ProcessChange {
    pub fn destructive(&self) -> bool {
        !matches!(self.observed, ObservedForeground::Idle)
    }
}

pub struct ProcessPlan {
    panes: Vec<PlannedPane>,
    live_topologies: Vec<WorkspaceTopology>,
    restored_topologies: Vec<SnapshotWorkspace>,
}

struct PlannedPane {
    workspace: String,
    window: u32,
    pane: usize,
    path: String,
    policy: RestartPolicy,
    shell: Option<SavedShell>,
    command: Option<String>,
    warning: Option<String>,
    expected: Option<(String, ObservedForeground)>,
}

impl Snapshot {
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::new();
        field(&mut output, "tmux-atelier-restore");
        field(&mut output, "3");
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
                field(&mut output, &window.panes.len().to_string());
                field(&mut output, &window.active_pane.to_string());
                for pane in &window.panes {
                    field(&mut output, "pane");
                    field(&mut output, &pane.path);
                    field(&mut output, pane.policy.as_str());
                    if let Some(shell) = &pane.shell {
                        field(&mut output, &shell.executable);
                        field(&mut output, if shell.login { "login" } else { "normal" });
                    } else {
                        field(&mut output, "");
                        field(&mut output, "normal");
                    }
                    if let Some(process) = &pane.process {
                        field(&mut output, &process.executable);
                        field(&mut output, &process.argv.len().to_string());
                        for argument in &process.argv {
                            field(&mut output, argument);
                        }
                    } else {
                        field(&mut output, "");
                        field(&mut output, "0");
                    }
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
        let version = next(&mut tokens)?;
        if !matches!(version.as_str(), "2" | "3") {
            return Err(err("invalid restore snapshot"));
        }
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
                let mut panes = Vec::with_capacity(pane_count);
                for _ in 0..pane_count {
                    expect(&mut tokens, "pane")?;
                    let path = next(&mut tokens)?;
                    panes.push(if version == "2" {
                        SnapshotPane {
                            path,
                            policy: RestartPolicy::Auto,
                            shell: None,
                            process: None,
                        }
                    } else {
                        decode_pane(&mut tokens, path)?
                    });
                }
                expect(&mut tokens, "end-window")?;
                windows.push(SnapshotWindow {
                    index,
                    name: window_name,
                    automatic_rename,
                    layout,
                    active_pane,
                    panes,
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
            .map(|window| window.panes.len())
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

fn decode_pane(tokens: &mut impl Iterator<Item = String>, path: String) -> Result<SnapshotPane> {
    let policy =
        RestartPolicy::parse(&next(tokens)?).map_err(|_| err("invalid restore snapshot"))?;
    let shell_executable = next(tokens)?;
    let login = match next(tokens)?.as_str() {
        "login" => true,
        "normal" => false,
        _ => return Err(err("invalid restore snapshot")),
    };
    let process_executable = next(tokens)?;
    let argument_count = number(tokens)? as usize;
    let mut argv = Vec::with_capacity(argument_count);
    for _ in 0..argument_count {
        argv.push(next(tokens)?);
    }
    if process_executable.is_empty() != argv.is_empty() {
        return Err(err("invalid restore snapshot"));
    }
    Ok(SnapshotPane {
        path,
        policy,
        shell: (!shell_executable.is_empty()).then_some(SavedShell {
            executable: shell_executable,
            login,
        }),
        process: (!process_executable.is_empty()).then_some(SavedProcess {
            executable: process_executable,
            argv,
        }),
    })
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
        } else if path == config.restore_lock {
            "could not acquire restore lock"
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
    let inspector = process_state::poll_interval(config)?
        .map(|_| ProcessInspector::new())
        .transpose()?;
    for name in sessions {
        if let Some(workspace) =
            capture_workspace(config, &name, exclude_window, inspector.as_ref())?
        {
            saved.push(workspace);
        }
    }
    let snapshot = Snapshot {
        active,
        workspaces: saved,
    };
    let encoded = snapshot.encode();
    if fs::read(&config.restore_file).is_ok_and(|current| current == encoded) {
        return Ok(());
    }
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
        file.write_all(&encoded)?;
        file.sync_all()?;
        fs::rename(&temporary, &config.restore_file)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn current_workspace(config: &Config, name: &str) -> Result<Option<WorkspaceTopology>> {
    if !workspace::session_exists(config, name)
        || workspace::session_option(config, name, "@atelier_managed") != "1"
        || !config.workspaces.join(name).is_file()
    {
        return Ok(None);
    }
    capture_topology(config, name, "")
}

pub fn workspace_matches(config: &Config, saved: &SnapshotWorkspace) -> Result<bool> {
    let Some(current) = current_workspace(config, &saved.name)? else {
        return Ok(false);
    };
    let definition = workspace::read(config, &saved.name)?;
    Ok(topologies_match(
        &current,
        &saved.topology(),
        (definition.destination == "local").then_some(definition.path.as_str()),
    ))
}

pub fn topology_unchanged(left: &WorkspaceTopology, right: &WorkspaceTopology) -> bool {
    topologies_match_exact(left, right)
}

pub fn topology_matches_snapshot_at(
    current: &WorkspaceTopology,
    saved: &SnapshotWorkspace,
    fallback_path: Option<&str>,
) -> bool {
    topologies_match(current, &saved.topology(), fallback_path)
}

pub fn process_changes(config: &Config, snapshot: &Snapshot) -> Result<Vec<ProcessChange>> {
    if process_state::poll_interval(config)?.is_none() {
        return Ok(Vec::new());
    }
    let mut changes = Vec::new();
    let inspector = ProcessInspector::new()?;
    for workspace in &snapshot.workspaces {
        if !config.workspaces.join(&workspace.name).is_file() {
            continue;
        }
        let topology_matches = workspace::session_exists(config, &workspace.name)
            && workspace_matches(config, workspace)?;
        for window in &workspace.windows {
            if !topology_matches {
                for (ordinal, pane) in window.panes.iter().enumerate() {
                    if let Some(saved) = &pane.process {
                        changes.push(ProcessChange {
                            key: process_key(&workspace.name, window.index, ordinal),
                            workspace: workspace.name.clone(),
                            window: window.index,
                            pane: ordinal,
                            program: saved
                                .argv
                                .first()
                                .cloned()
                                .unwrap_or_else(|| saved.executable.clone()),
                            current: None,
                            observed: ObservedForeground::Idle,
                        });
                    }
                }
                continue;
            }
            let panes = window_panes(config, &workspace.name, window.index)?;
            for (ordinal, (pane_id, pane)) in panes.iter().zip(&window.panes).enumerate() {
                let Some(saved) = &pane.process else {
                    continue;
                };
                let current = inspector.inspect(config, pane_id)?.foreground;
                if matches!(&current, ObservedForeground::Process(process) if process.argv == saved.argv)
                {
                    continue;
                }
                changes.push(ProcessChange {
                    key: process_key(&workspace.name, window.index, ordinal),
                    workspace: workspace.name.clone(),
                    window: window.index,
                    pane: ordinal,
                    program: saved
                        .argv
                        .first()
                        .cloned()
                        .unwrap_or_else(|| saved.executable.clone()),
                    current: match &current {
                        ObservedForeground::Idle => None,
                        ObservedForeground::Process(process) => process.argv.first().cloned(),
                        ObservedForeground::Busy(_) => Some("unidentified foreground group".into()),
                    },
                    observed: current,
                });
            }
        }
    }
    Ok(changes)
}

pub fn plan_processes(
    config: &Config,
    snapshot: &Snapshot,
    approved_replacements: &HashMap<String, ObservedForeground>,
) -> Result<ProcessPlan> {
    let enabled = process_state::poll_interval(config)?.is_some();
    let inspector = enabled.then(ProcessInspector::new).transpose()?;
    let mut planned = Vec::new();
    let mut live_topologies = Vec::new();
    let mut restored_topologies = Vec::new();
    for workspace in &snapshot.workspaces {
        if !config.workspaces.join(&workspace.name).is_file() {
            continue;
        }
        let definition = workspace::read(config, &workspace.name)?;
        if definition.destination != "local" {
            continue;
        }
        let live_matches = workspace::session_exists(config, &workspace.name)
            && workspace_matches(config, workspace)?;
        if live_matches {
            if let Some(topology) = current_workspace(config, &workspace.name)? {
                live_topologies.push(topology);
            }
        } else {
            restored_topologies.push(workspace.clone());
        }
        for window in &workspace.windows {
            let pane_ids = if live_matches {
                window_panes(config, &workspace.name, window.index)?
            } else {
                Vec::new()
            };
            for (ordinal, pane) in window.panes.iter().enumerate() {
                let mut command = None;
                let mut warning = None;
                let mut expected = None;
                if enabled && let Some(shell) = &pane.shell {
                    let already_running = if let (Some(inspector), Some(saved), Some(pane_id)) =
                        (&inspector, &pane.process, pane_ids.get(ordinal))
                    {
                        let current = inspector.inspect(config, pane_id)?.foreground;
                        expected = Some((pane_id.clone(), current.clone()));
                        if matches!(&current, ObservedForeground::Process(process) if process.argv == saved.argv)
                        {
                            true
                        } else {
                            let key = process_key(&workspace.name, window.index, ordinal);
                            if !matches!(&current, ObservedForeground::Idle)
                                && approved_replacements.get(&key) != Some(&current)
                            {
                                return Err(err(format!(
                                    "pane process changed after confirmation: {key}"
                                )));
                            }
                            false
                        }
                    } else {
                        false
                    };
                    if !already_running {
                        if let Some(saved) = &pane.process {
                            match process_state::restart_disposition(config, shell, saved)? {
                                process_state::RestartDisposition::Runnable(value) => {
                                    command = Some(value)
                                }
                                process_state::RestartDisposition::ShellOnly {
                                    command: value,
                                    warning: value_warning,
                                } => {
                                    command = Some(value);
                                    warning = Some(value_warning);
                                }
                            }
                        } else if !live_matches {
                            command = Some(process_state::shell_command(shell));
                        }
                    }
                }
                let path = if Path::new(&pane.path).is_dir() {
                    pane.path.clone()
                } else {
                    definition.path.clone()
                };
                planned.push(PlannedPane {
                    workspace: workspace.name.clone(),
                    window: window.index,
                    pane: ordinal,
                    path,
                    policy: pane.policy,
                    shell: enabled.then(|| pane.shell.clone()).flatten(),
                    command,
                    warning,
                    expected,
                });
            }
        }
    }
    Ok(ProcessPlan {
        panes: planned,
        live_topologies,
        restored_topologies,
    })
}

pub fn apply_processes(config: &Config, plan: ProcessPlan) -> Vec<String> {
    let validation = (|| -> Result<Vec<String>> {
        for expected in &plan.live_topologies {
            let Some(current) = current_workspace(config, &expected.name)? else {
                return Err(err("live workspace changed before process restoration"));
            };
            if !topology_unchanged(&current, expected) {
                return Err(err("live workspace changed before process restoration"));
            }
        }
        for expected in &plan.restored_topologies {
            if !workspace_matches(config, expected)? {
                return Err(err("restored workspace changed before process restoration"));
            }
        }
        let inspector = plan
            .panes
            .iter()
            .any(|pane| pane.expected.is_some())
            .then(ProcessInspector::new)
            .transpose()?;
        let mut pane_ids = Vec::with_capacity(plan.panes.len());
        for pane in &plan.panes {
            let panes = window_panes(config, &pane.workspace, pane.window)?;
            let pane_id = panes
                .get(pane.pane)
                .ok_or_else(|| err("restored pane is missing"))?;
            if let Some((expected_id, expected_foreground)) = &pane.expected
                && (pane_id != expected_id
                    || inspector
                        .as_ref()
                        .ok_or_else(|| err("process inspector is unavailable"))?
                        .inspect(config, pane_id)?
                        .foreground
                        != *expected_foreground)
            {
                return Err(err("pane changed before process restoration"));
            }
            pane_ids.push(pane_id.clone());
        }
        Ok(pane_ids)
    })();
    let pane_ids = match validation {
        Ok(pane_ids) => pane_ids,
        Err(error) => return vec![format!("skipped process restoration: {error}")],
    };
    let mut failures = Vec::new();
    for (pane, pane_id) in plan.panes.into_iter().zip(pane_ids) {
        if pane.command.is_some()
            && let Some((expected_id, expected_foreground)) = &pane.expected
        {
            let current =
                ProcessInspector::new().and_then(|inspector| inspector.inspect(config, &pane_id));
            if pane_id != *expected_id
                || !matches!(current, Ok(runtime) if runtime.foreground == *expected_foreground)
            {
                failures.push(format!(
                    "skipped {}:{}.{} because the pane changed",
                    pane.workspace, pane.window, pane.pane
                ));
                continue;
            }
        }
        let result = (|| -> Result<()> {
            process_state::set_pane_policy(config, &pane_id, pane.policy)?;
            if let Some(command) = &pane.command {
                process::tmux(
                    config,
                    &[
                        "respawn-pane",
                        "-k",
                        "-t",
                        &pane_id,
                        "-c",
                        &pane.path,
                        command,
                    ],
                )?;
            }
            if let Some(shell) = &pane.shell {
                process_state::set_shell_override(config, &pane_id, shell)?;
            }
            Ok(())
        })();
        if let Some(warning) = pane.warning {
            failures.push(warning);
        }
        if let Err(error) = result {
            failures.push(format!(
                "could not restore {}:{}.{}: {error}",
                pane.workspace, pane.window, pane.pane
            ));
        }
    }
    failures
}

fn process_key(workspace: &str, window: u32, pane: usize) -> String {
    format!("{workspace}:{window}.{pane}")
}

fn window_panes(config: &Config, workspace: &str, window: u32) -> Result<Vec<String>> {
    Ok(lines(process::tmux_output(
        config,
        &[
            "list-panes",
            "-t",
            &format!("={workspace}:{window}"),
            "-F",
            "#{pane_id}",
        ],
    )?))
}

fn capture_workspace(
    config: &Config,
    name: &str,
    exclude_window: &str,
    inspector: Option<&ProcessInspector>,
) -> Result<Option<SnapshotWorkspace>> {
    let Some(topology) = capture_topology(config, name, exclude_window)? else {
        return Ok(None);
    };
    let destination = workspace::session_option(config, name, "@atelier_destination");
    let mut windows = Vec::with_capacity(topology.windows.len());
    for window in topology.windows {
        let pane_ids = window_panes(config, name, window.index)?;
        let mut panes = Vec::with_capacity(window.panes.len());
        for (pane, pane_id) in window.panes.into_iter().zip(pane_ids) {
            panes.push(if destination == "local" {
                let (policy, shell, process) = if let Some(inspector) = inspector {
                    let runtime = inspector.inspect(config, &pane_id)?;
                    (runtime.policy, runtime.shell, runtime.restartable)
                } else {
                    (process_state::pane_policy(config, &pane_id)?, None, None)
                };
                SnapshotPane {
                    path: pane.path,
                    policy,
                    shell,
                    process,
                }
            } else {
                SnapshotPane {
                    path: pane.path,
                    policy: RestartPolicy::Auto,
                    shell: None,
                    process: None,
                }
            });
        }
        windows.push(SnapshotWindow {
            index: window.index,
            name: window.name,
            automatic_rename: window.automatic_rename,
            layout: window.layout,
            active_pane: window.active_pane,
            panes,
        });
    }
    Ok(Some(SnapshotWorkspace {
        name: topology.name,
        active_window: topology.active_window,
        windows,
    }))
}

fn capture_topology(
    config: &Config,
    name: &str,
    exclude_window: &str,
) -> Result<Option<WorkspaceTopology>> {
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
        return Ok(None);
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
    let destination = workspace::session_option(config, name, "@atelier_destination");
    let mut saved_windows = Vec::new();
    for window in windows {
        let panes = lines(process::tmux_output(
            config,
            &["list-panes", "-t", &window, "-F", "#{pane_id}"],
        )?);
        let mut active_pane = 0;
        let mut saved_panes = Vec::new();
        for (index, pane) in panes.iter().enumerate() {
            if process::tmux_output(
                config,
                &["display-message", "-p", "-t", pane, "#{pane_active}"],
            )? == "1"
            {
                active_pane = index;
            }
            saved_panes.push(PaneTopology {
                path: if destination == "local" {
                    process::tmux_output(
                        config,
                        &["display-message", "-p", "-t", pane, "#{pane_current_path}"],
                    )?
                } else {
                    String::new()
                },
            });
        }
        saved_windows.push(WindowTopology {
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
            panes: saved_panes,
        });
    }
    Ok(Some(WorkspaceTopology {
        name: name.into(),
        active_window,
        windows: saved_windows,
    }))
}

fn topologies_match(
    current: &WorkspaceTopology,
    saved: &WorkspaceTopology,
    fallback_path: Option<&str>,
) -> bool {
    current.name == saved.name
        && current.active_window == saved.active_window
        && current.windows.len() == saved.windows.len()
        && current
            .windows
            .iter()
            .zip(&saved.windows)
            .all(|(current, saved)| {
                current.index == saved.index
                    && current.automatic_rename == saved.automatic_rename
                    && (current.automatic_rename || current.name == saved.name)
                    && current.active_pane == saved.active_pane
                    && current.panes.len() == saved.panes.len()
                    && current
                        .panes
                        .iter()
                        .zip(&saved.panes)
                        .all(|(current, saved)| {
                            current.path == saved.path
                                || fallback_path.is_some_and(|fallback| {
                                    !Path::new(&saved.path).is_dir() && current.path == fallback
                                })
                        })
                    && layouts_match(&current.layout, &saved.layout)
            })
}

fn topologies_match_exact(left: &WorkspaceTopology, right: &WorkspaceTopology) -> bool {
    left.name == right.name
        && left.active_window == right.active_window
        && left.windows.len() == right.windows.len()
        && left
            .windows
            .iter()
            .zip(&right.windows)
            .all(|(left, right)| {
                left.index == right.index
                    && left.automatic_rename == right.automatic_rename
                    && (left.automatic_rename || left.name == right.name)
                    && left.active_pane == right.active_pane
                    && left.panes == right.panes
                    && parsed_layout(&left.layout)
                        .zip(parsed_layout(&right.layout))
                        .is_some_and(|(left, right)| left == right)
            })
}

fn layouts_match(left: &str, right: &str) -> bool {
    parsed_layout(left)
        .zip(parsed_layout(right))
        .is_some_and(|(left, right)| layout_cells_match(&left, &right))
}

fn parsed_layout(layout: &str) -> Option<LayoutCell> {
    let (_, body) = layout.split_once(',')?;
    let mut parser = LayoutParser {
        input: body.as_bytes(),
        position: 0,
    };
    let cell = parser.cell()?;
    (parser.position == parser.input.len()).then_some(cell)
}

#[derive(Eq, PartialEq)]
struct LayoutCell {
    width: u32,
    height: u32,
    split: Option<(u8, Vec<LayoutCell>)>,
}

fn layout_cells_match(left: &LayoutCell, right: &LayoutCell) -> bool {
    match (&left.split, &right.split) {
        (None, None) => true,
        (Some((left_kind, left_children)), Some((right_kind, right_children)))
            if left_kind == right_kind && left_children.len() == right_children.len() =>
        {
            let left_total: u32 = left_children
                .iter()
                .map(|child| child.dimension(*left_kind))
                .sum();
            let right_total: u32 = right_children
                .iter()
                .map(|child| child.dimension(*right_kind))
                .sum();
            left_children
                .iter()
                .zip(right_children)
                .all(|(left, right)| {
                    proportions_match(
                        left.dimension(*left_kind),
                        left_total,
                        right.dimension(*right_kind),
                        right_total,
                    ) && layout_cells_match(left, right)
                })
        }
        _ => false,
    }
}

impl LayoutCell {
    fn dimension(&self, split: u8) -> u32 {
        if split == b'{' {
            self.width
        } else {
            self.height
        }
    }
}

fn proportions_match(left: u32, left_total: u32, right: u32, right_total: u32) -> bool {
    let left_scaled = u64::from(left) * u64::from(right_total);
    let right_scaled = u64::from(right) * u64::from(left_total);
    left_scaled.abs_diff(right_scaled) * 100 <= u64::from(left_total) * u64::from(right_total) * 2
}

struct LayoutParser<'a> {
    input: &'a [u8],
    position: usize,
}

impl LayoutParser<'_> {
    fn cell(&mut self) -> Option<LayoutCell> {
        let width = self.number()?;
        self.byte(b'x')?;
        let height = self.number()?;
        self.byte(b',')?;
        self.number()?;
        self.byte(b',')?;
        self.number()?;
        let split = match self.input.get(self.position).copied() {
            Some(b',') => {
                self.position += 1;
                self.number()?;
                while self
                    .input
                    .get(self.position)
                    .is_some_and(u8::is_ascii_alphabetic)
                {
                    self.position += 1;
                }
                None
            }
            Some(open @ (b'{' | b'[')) => {
                self.position += 1;
                let close = if open == b'{' { b'}' } else { b']' };
                let mut children = Vec::new();
                loop {
                    children.push(self.cell()?);
                    match self.input.get(self.position).copied()? {
                        byte if byte == close => {
                            self.position += 1;
                            break;
                        }
                        b',' => {
                            self.position += 1;
                        }
                        _ => return None,
                    }
                }
                Some((open, children))
            }
            None | Some(b'}' | b']') => None,
            _ => return None,
        };
        Some(LayoutCell {
            width,
            height,
            split,
        })
    }

    fn number(&mut self) -> Option<u32> {
        let start = self.position;
        while self
            .input
            .get(self.position)
            .is_some_and(u8::is_ascii_digit)
        {
            self.position += 1;
        }
        if self.position == start {
            return None;
        }
        std::str::from_utf8(&self.input[start..self.position])
            .ok()?
            .parse()
            .ok()
    }

    fn byte(&mut self, wanted: u8) -> Option<()> {
        if self.input.get(self.position) != Some(&wanted) {
            return None;
        }
        self.position += 1;
        Some(())
    }
}

fn lines(value: String) -> Vec<String> {
    value
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn restore(
    config: &Config,
    snapshot: &Snapshot,
    approved_replacements: &HashMap<String, WorkspaceTopology>,
) -> Result<()> {
    let mut missing = Vec::new();
    let mut replacements = Vec::new();
    let mut reserved_names = reserved_replacement_names(config);
    for saved in &snapshot.workspaces {
        if !config.workspaces.join(&saved.name).is_file() {
            continue;
        }
        let definition = workspace::read(config, &saved.name)?;
        if !workspace::session_exists(config, &saved.name) {
            missing.push((definition, saved.clone()));
        } else if !workspace_matches(config, saved)? {
            let Some(approved) = approved_replacements.get(&saved.name) else {
                return Err(err(format!(
                    "workspace replacement was not confirmed: {}",
                    saved.name
                )));
            };
            let Some(current) = current_workspace(config, &saved.name)? else {
                return Err(err(format!(
                    "workspace changed after replacement confirmation: {}",
                    saved.name
                )));
            };
            if !topology_unchanged(&current, approved) {
                return Err(err(format!(
                    "workspace changed after replacement confirmation: {}",
                    saved.name
                )));
            }
            let staged_name = replacement_name(config, &mut reserved_names);
            let backup = replacement_name(config, &mut reserved_names);
            let mut staged_definition = definition.clone();
            staged_definition.name.clone_from(&staged_name);
            replacements.push(Replacement {
                original: saved.name.clone(),
                staged: staged_name,
                backup,
                staged_definition,
                saved: saved.clone(),
                approved: approved.clone(),
            });
        }
    }

    let generation = replacement_generation();
    begin_transaction(config, &generation)?;
    let result = (|| -> Result<()> {
        for replacement in &replacements {
            set_replacement_record(config, &generation, replacement)?;
            restore_workspace(
                config,
                &replacement.staged_definition,
                &replacement.saved,
                &generation,
            )?;
        }
        for (definition, saved) in &missing {
            restore_workspace(config, definition, saved, &generation)?;
        }
        for replacement in &replacements {
            let Some(current) = current_workspace(config, &replacement.original)? else {
                return Err(err(format!(
                    "workspace changed after replacement confirmation: {}",
                    replacement.original
                )));
            };
            if !topology_unchanged(&current, &replacement.approved) {
                return Err(err(format!(
                    "workspace changed after replacement confirmation: {}",
                    replacement.original
                )));
            }
            swap_replacement(config, replacement)?;
        }
        Ok(())
    })();
    if result.is_err() {
        recover_replacements(config)?;
    }
    result
}

const REPLACEMENT_OPTION: &str = "@atelier_restore_transaction";
const TRANSACTION_OPTION: &str = "@atelier_restore_transaction_phase";
const OWNER_OPTION: &str = "@atelier_restore_owner";

fn set_replacement_record(
    config: &Config,
    generation: &str,
    replacement: &Replacement,
) -> Result<()> {
    process::tmux(
        config,
        &[
            "set-option",
            "-q",
            "-t",
            &format!("={}:", replacement.original),
            REPLACEMENT_OPTION,
            &format!(
                "2|{generation}|{}|{}|{}",
                replacement.original, replacement.staged, replacement.backup
            ),
        ],
    )
}

fn clear_replacement_record(config: &Config, session: &str) -> Result<()> {
    process::tmux(
        config,
        &[
            "set-option",
            "-qu",
            "-t",
            &format!("={session}:"),
            REPLACEMENT_OPTION,
        ],
    )
}

fn replacement_record(value: &str) -> Result<(&str, &str, &str, &str)> {
    let fields: Vec<_> = value.split('|').collect();
    if fields.len() != 5 || fields[0] != "2" || fields[1].is_empty() {
        return Err(err("invalid replacement recovery record"));
    }
    for name in &fields[2..] {
        workspace::validate_name(name).map_err(|_| err("invalid replacement recovery record"))?;
    }
    Ok((fields[1], fields[2], fields[3], fields[4]))
}

pub fn recover_replacements(config: &Config) -> Result<()> {
    let phase = process::tmux_quiet(config, &["show-options", "-gqv", TRANSACTION_OPTION])
        .unwrap_or_default();
    if let Some((generation, "committed")) = phase.split_once('|') {
        let mut failures = cleanup_committed(config, generation);
        failures.extend(normalize_restore_state(config));
        if failures.is_empty()
            && let Err(error) = clear_transaction_phase(config)
        {
            failures.push(format!("could not clear restore transaction: {error}"));
        }
        return failures
            .is_empty()
            .then_some(())
            .ok_or_else(|| err(failures.join("; ")));
    }
    let prepared_generation = phase
        .split_once('|')
        .filter(|(_, state)| *state == "prepared")
        .map(|(generation, _)| generation.to_owned());
    for holder in workspace::session_names(config) {
        let value = workspace::session_option(config, &holder, REPLACEMENT_OPTION);
        if value.is_empty() {
            continue;
        }
        let (generation, original, staged, _) = replacement_record(&value)?;
        if prepared_generation
            .as_deref()
            .is_some_and(|prepared| prepared != generation)
        {
            return Err(err("replacement recovery generation does not match"));
        }
        if holder != original {
            if workspace::session_exists(config, original)
                && session_owned(config, original, generation)
            {
                process::tmux(config, &["kill-session", "-t", &format!("={original}")])?;
            }
            if staged != original
                && workspace::session_exists(config, staged)
                && session_owned(config, staged, generation)
            {
                process::tmux(config, &["kill-session", "-t", &format!("={staged}")])?;
            }
            process::tmux(
                config,
                &["rename-session", "-t", &format!("={holder}"), original],
            )?;
        } else if workspace::session_exists(config, staged)
            && session_owned(config, staged, generation)
        {
            process::tmux(config, &["kill-session", "-t", &format!("={staged}")])?;
        }
        clear_replacement_record(config, original)?;
    }
    if let Some(generation) = prepared_generation {
        for session in workspace::session_names(config) {
            if session_owned(config, &session, &generation) {
                process::tmux(config, &["kill-session", "-t", &format!("={session}")])?;
            }
        }
    }
    clear_transaction_phase(config)?;
    Ok(())
}

pub fn commit_replacements(config: &Config) -> Result<Vec<String>> {
    let phase = process::tmux_output(config, &["show-options", "-gqv", TRANSACTION_OPTION])?;
    if phase.is_empty() {
        return Ok(Vec::new());
    }
    let Some((generation, "prepared")) = phase.split_once('|') else {
        return Err(err("replacement transaction is not prepared"));
    };
    set_transaction_phase(config, generation, "committed")?;
    Ok(cleanup_committed(config, generation))
}

pub fn finish_replacements(config: &Config, state_normalized: bool) -> Vec<String> {
    let phase = process::tmux_quiet(config, &["show-options", "-gqv", TRANSACTION_OPTION])
        .unwrap_or_default();
    let Some((generation, "committed")) = phase.split_once('|') else {
        return Vec::new();
    };
    let mut failures = cleanup_committed(config, generation);
    if state_normalized
        && failures.is_empty()
        && let Err(error) = clear_transaction_phase(config)
    {
        failures.push(format!("could not clear restore transaction: {error}"));
    }
    failures
}

fn cleanup_committed(config: &Config, generation: &str) -> Vec<String> {
    let mut failures = Vec::new();
    for holder in workspace::session_names(config) {
        let value = workspace::session_option(config, &holder, REPLACEMENT_OPTION);
        if value.is_empty() {
            continue;
        }
        let Ok((record_generation, original, _, backup)) = replacement_record(&value) else {
            failures.push("invalid replacement recovery record".into());
            continue;
        };
        if record_generation != generation {
            failures.push("replacement recovery generation does not match".into());
            continue;
        }
        if holder != backup || !workspace::session_exists(config, original) {
            failures.push(format!("replacement did not commit: {original}"));
            continue;
        }
        if let Err(error) = process::tmux(config, &["kill-session", "-t", &format!("={holder}")]) {
            failures.push(format!(
                "could not delete replacement backup {holder}: {error}"
            ));
        }
    }
    for session in workspace::session_names(config) {
        if session_owned(config, &session, generation) {
            let result = process::tmux(
                config,
                &[
                    "set-option",
                    "-qu",
                    "-t",
                    &format!("={session}:"),
                    OWNER_OPTION,
                ],
            );
            if let Err(error) = result {
                failures.push(format!(
                    "could not clear restore owner on {session}: {error}"
                ));
            }
        }
    }
    failures
}

fn begin_transaction(config: &Config, generation: &str) -> Result<()> {
    let current = process::tmux_quiet(config, &["show-options", "-gqv", TRANSACTION_OPTION])
        .unwrap_or_default();
    if !current.is_empty() {
        return Err(err("another restore transaction is active"));
    }
    set_transaction_phase(config, generation, "prepared")
}

fn set_transaction_phase(config: &Config, generation: &str, phase: &str) -> Result<()> {
    process::tmux(
        config,
        &[
            "set-option",
            "-gq",
            TRANSACTION_OPTION,
            &format!("{generation}|{phase}"),
        ],
    )
}

fn clear_transaction_phase(config: &Config) -> Result<()> {
    process::tmux(config, &["set-option", "-gu", TRANSACTION_OPTION])
}

fn session_owned(config: &Config, session: &str, generation: &str) -> bool {
    workspace::session_option(config, session, OWNER_OPTION) == generation
}

fn normalize_restore_state(config: &Config) -> Vec<String> {
    let mut failures = Vec::new();
    for (option, value) in [
        ("@atelier_restore_pending", "0"),
        ("@atelier_restore_handled", "1"),
        ("@atelier_restore_started", "1"),
    ] {
        if let Err(error) = process::tmux(config, &["set-option", "-gq", option, value]) {
            failures.push(format!("could not set {option}: {error}"));
        }
    }
    failures
}

struct Replacement {
    original: String,
    staged: String,
    backup: String,
    staged_definition: Workspace,
    saved: SnapshotWorkspace,
    approved: WorkspaceTopology,
}

fn swap_replacement(config: &Config, replacement: &Replacement) -> Result<()> {
    let backup = &replacement.backup;
    process::tmux(
        config,
        &[
            "rename-session",
            "-t",
            &format!("={}", replacement.original),
            backup,
        ],
    )?;
    if process::tmux(
        config,
        &[
            "rename-session",
            "-t",
            &format!("={}", replacement.staged),
            &replacement.original,
        ],
    )
    .is_err()
    {
        let _ = process::tmux(
            config,
            &[
                "rename-session",
                "-t",
                &format!("={backup}"),
                &replacement.original,
            ],
        );
        return Err(err(format!(
            "could not replace workspace session: {}",
            replacement.original
        )));
    }
    Ok(())
}

fn replacement_generation() -> String {
    format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

fn reserved_replacement_names(config: &Config) -> HashSet<String> {
    workspace::session_names(config)
        .into_iter()
        .flat_map(|session| {
            workspace::session_option(config, &session, REPLACEMENT_OPTION)
                .split('|')
                .skip(1)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn replacement_name(config: &Config, reserved: &mut HashSet<String>) -> String {
    let mut suffix = 1;
    loop {
        let name = format!("atelier-restore-{suffix}");
        if !reserved.contains(&name)
            && !workspace::session_exists(config, &name)
            && !config.workspaces.join(&name).exists()
        {
            reserved.insert(name.clone());
            return name;
        }
        suffix += 1;
    }
}

fn restore_workspace(
    config: &Config,
    definition: &Workspace,
    saved: &SnapshotWorkspace,
    generation: &str,
) -> Result<()> {
    let mut restored_active = String::new();
    for (ordinal, saved_window) in saved.windows.iter().enumerate() {
        let saved_first_path = saved_window
            .panes
            .first()
            .map(|pane| pane.path.as_str())
            .unwrap_or(&definition.path);
        let first_path =
            if definition.destination == "local" && !Path::new(saved_first_path).is_dir() {
                &definition.path
            } else {
                saved_first_path
            };
        let window = if ordinal == 0 {
            workspace::create_restore_session(config, definition, first_path, generation)?;
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
        let current_index: u32 = process::tmux_output(
            config,
            &["display-message", "-p", "-t", &window, "#{window_index}"],
        )?
        .parse()?;
        if current_index != saved_window.index {
            process::tmux(
                config,
                &[
                    "move-window",
                    "-s",
                    &window,
                    "-t",
                    &format!("={}:{}", definition.name, saved_window.index),
                ],
            )?;
        }
        for saved_pane in saved_window.panes.iter().skip(1) {
            let path = &saved_pane.path;
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
                    panes: vec![SnapshotPane {
                        path: "/tmp/a path".into(),
                        policy: RestartPolicy::Always,
                        shell: Some(SavedShell {
                            executable: "/bin/zsh".into(),
                            login: true,
                        }),
                        process: Some(SavedProcess {
                            executable: "/usr/bin/opencode".into(),
                            argv: vec!["opencode".into(), "--auto".into()],
                        }),
                    }],
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

    #[test]
    fn version_two_snapshot_decodes_without_process_state() {
        let bytes = b"tmux-atelier-restore\x002\0active\0main\0workspace\0main\x001\0window\x001\0shell\0off\0abcd,80x24,0,0,1\x001\x000\0pane\0/tmp\0end-window\0end-workspace\0end\0";
        let saved = Snapshot::decode(bytes).unwrap();
        assert_eq!(saved.workspaces[0].windows[0].panes[0].path, "/tmp");
        assert_eq!(
            saved.workspaces[0].windows[0].panes,
            [SnapshotPane {
                path: "/tmp".into(),
                policy: RestartPolicy::Auto,
                shell: None,
                process: None,
            }]
        );
    }

    #[test]
    fn layout_comparison_ignores_checksum_and_pane_ids() {
        assert!(layouts_match(
            "aaaa,80x24,0,0{39x24,0,0,1,40x24,40,0,2}",
            "bbbb,120x36,0,0{59x36,0,0,17,60x36,60,0,29}"
        ));
        assert!(!layouts_match(
            "aaaa,80x24,0,0{39x24,0,0,1,40x24,40,0,2}",
            "bbbb,80x24,0,0[80x11,0,0,17,80x12,0,12,29]"
        ));
    }

    #[test]
    fn stale_approval_ignores_only_generated_names_and_runtime_layout_ids() {
        let original = WorkspaceTopology {
            name: "main".into(),
            active_window: 0,
            windows: vec![WindowTopology {
                index: 0,
                name: "tmux".into(),
                automatic_rename: true,
                layout: "aaaa,80x24,0,0,1".into(),
                active_pane: 0,
                panes: vec![PaneTopology {
                    path: "/tmp".into(),
                }],
            }],
        };
        let mut current = original.clone();
        current.windows[0].name = "zsh".into();
        current.windows[0].layout = "bbbb,80x24,0,0,99".into();
        assert!(topology_unchanged(&original, &current));

        current.windows[0].automatic_rename = false;
        assert!(!topology_unchanged(&original, &current));
        let mut explicitly_named = original.clone();
        explicitly_named.windows[0].automatic_rename = false;
        let mut renamed = explicitly_named.clone();
        renamed.windows[0].name = "other".into();
        assert!(!topology_unchanged(&explicitly_named, &renamed));
        current = original.clone();
        current.windows[0].panes[0].path = "/var/tmp".into();
        assert!(!topology_unchanged(&original, &current));
        current = original.clone();
        current.windows[0].layout = "bbbb,81x24,0,0,99".into();
        assert!(!topology_unchanged(&original, &current));
    }
}
