use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::config::{Config, quote_sh};
use crate::{Result, err, process};
use clap::ValueEnum;

pub const DEFAULT_DENYLIST: &str = "awk bash basename cat chmod chown cmake cp curl cut date dd diff dirname du echo env false fd find fish fzf git go grep head install kill less ln ls make man mkdir mv ninja node npm pacman pnpm printf pwd python python3 readlink realpath rg rm rmdir rsync ruby scp sed sh sleep sort ssh stat tail tar tee test tmux touch tr true uname uniq wc wget xargs zsh";

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum RestartPolicy {
    Auto,
    Always,
    Never,
}

impl RestartPolicy {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "" | "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err(err(
                "invalid restart policy: expected auto, always, or never",
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedShell {
    pub executable: String,
    pub login: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SavedProcess {
    pub executable: String,
    pub argv: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PaneRuntime {
    pub shell: Option<SavedShell>,
    pub foreground: ObservedForeground,
    pub restartable: Option<SavedProcess>,
    pub policy: RestartPolicy,
    pub capture: CaptureDecision,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureDecision {
    Idle,
    Busy {
        processes: usize,
    },
    Never,
    TooYoung {
        runtime: Duration,
        minimum: Duration,
    },
    Denylisted {
        executable: String,
    },
    Restartable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObservedForeground {
    Idle,
    Process(SavedProcess),
    Busy(Vec<SavedProcess>),
}

pub enum RestartDisposition {
    Runnable(String),
    ShellOnly { command: String, warning: String },
}

#[derive(Clone, Debug)]
struct ProcEntry {
    ppid: i32,
    pgrp: i32,
    tty: i32,
    tpgid: i32,
    start_ticks: u64,
    executable: String,
    argv: Vec<String>,
}

pub struct ProcessInspector {
    processes: HashMap<i32, ProcEntry>,
}

impl ProcessInspector {
    pub fn new() -> Result<Self> {
        let mut processes = HashMap::new();
        for entry in fs::read_dir("/proc")? {
            let entry = entry?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            if let Ok(process) = read_entry(pid) {
                processes.insert(pid, process);
            }
        }
        Ok(Self { processes })
    }

    pub fn inspect(&self, config: &Config, pane: &str) -> Result<PaneRuntime> {
        let policy = pane_policy(config, pane)?;
        let pane_pid: i32 = process::tmux_output(
            config,
            &["display-message", "-p", "-t", pane, "#{pane_pid}"],
        )?
        .parse()?;
        let shell_entry = self
            .processes
            .get(&pane_pid)
            .ok_or_else(|| err("pane process disappeared"))?;
        let detected_shell = SavedShell {
            executable: shell_entry.executable.clone(),
            login: shell_entry
                .argv
                .first()
                .is_some_and(|arg| arg.starts_with('-')),
        };
        let shell = Some(shell_override(config, pane)?.unwrap_or(detected_shell));
        if shell_entry.tpgid <= 0 || shell_entry.tpgid == shell_entry.pgrp {
            return Ok(PaneRuntime {
                shell,
                foreground: ObservedForeground::Idle,
                restartable: None,
                policy,
                capture: CaptureDecision::Idle,
            });
        }
        let group: HashMap<_, _> = self
            .processes
            .iter()
            .filter(|(_, process)| {
                process.tty == shell_entry.tty && process.pgrp == shell_entry.tpgid
            })
            .map(|(pid, process)| (*pid, process))
            .collect();
        let roots: Vec<_> = group
            .values()
            .filter(|entry| !group.contains_key(&entry.ppid))
            .collect();
        if roots.len() != 1 {
            let mut processes = group
                .values()
                .map(|process| SavedProcess {
                    executable: process.executable.clone(),
                    argv: process.argv.clone(),
                })
                .collect::<Vec<_>>();
            processes.sort_by(|left, right| left.argv.cmp(&right.argv));
            return Ok(PaneRuntime {
                shell,
                foreground: ObservedForeground::Busy(processes),
                restartable: None,
                policy,
                capture: CaptureDecision::Busy {
                    processes: group.len(),
                },
            });
        }
        let root = roots[0];
        let foreground = SavedProcess {
            executable: root.executable.clone(),
            argv: root.argv.clone(),
        };
        let (restartable, capture) = match policy {
            RestartPolicy::Never => (None, CaptureDecision::Never),
            RestartPolicy::Always => (Some(foreground.clone()), CaptureDecision::Restartable),
            RestartPolicy::Auto => {
                let minimum = minimum_runtime(config)?;
                let runtime = process_runtime(root.start_ticks)?;
                let denylist = process::tmux_quiet(
                    config,
                    &["show-options", "-gqv", "@atelier_restart_denylist"],
                )
                .unwrap_or_else(|| DEFAULT_DENYLIST.into());
                if runtime < minimum {
                    (None, CaptureDecision::TooYoung { runtime, minimum })
                } else if executable_is_denylisted(&root.executable, &denylist) {
                    (
                        None,
                        CaptureDecision::Denylisted {
                            executable: root.executable.clone(),
                        },
                    )
                } else {
                    (Some(foreground.clone()), CaptureDecision::Restartable)
                }
            }
        };
        Ok(PaneRuntime {
            shell,
            foreground: ObservedForeground::Process(foreground),
            restartable,
            policy,
            capture,
        })
    }
}

fn executable_is_denylisted(executable: &str, denylist: &str) -> bool {
    Path::new(executable)
        .file_name()
        .is_some_and(|name| denylist.split_ascii_whitespace().any(|item| name == item))
}

pub fn inspect(config: &Config, pane: &str) -> Result<PaneRuntime> {
    ProcessInspector::new()?.inspect(config, pane)
}

pub fn pane_policy(config: &Config, pane: &str) -> Result<RestartPolicy> {
    let value = process::tmux_quiet(
        config,
        &[
            "show-options",
            "-pqv",
            "-t",
            pane,
            "@atelier_restart_policy",
        ],
    )
    .unwrap_or_default();
    RestartPolicy::parse(&value)
}

pub fn set_pane_policy(config: &Config, pane: &str, policy: RestartPolicy) -> Result<()> {
    process::tmux(
        config,
        &[
            "set-option",
            "-pq",
            "-t",
            pane,
            "@atelier_restart_policy",
            policy.as_str(),
        ],
    )
}

pub fn set_shell_override(config: &Config, pane: &str, shell: &SavedShell) -> Result<()> {
    process::tmux(
        config,
        &[
            "set-option",
            "-pq",
            "-t",
            pane,
            "@atelier_restart_shell_state",
            &format!(
                "1|{}|{}",
                if shell.login { "1" } else { "0" },
                hex(shell.executable.as_bytes())
            ),
        ],
    )?;
    for option in ["@atelier_restart_shell", "@atelier_restart_shell_login"] {
        let _ = process::tmux(config, &["set-option", "-pqu", "-t", pane, option]);
    }
    Ok(())
}

fn shell_override(config: &Config, pane: &str) -> Result<Option<SavedShell>> {
    let state = process::tmux_quiet(
        config,
        &[
            "show-options",
            "-pqv",
            "-t",
            pane,
            "@atelier_restart_shell_state",
        ],
    )
    .unwrap_or_default();
    if !state.is_empty() {
        let fields: Vec<_> = state.split('|').collect();
        if fields.len() != 3 || fields[0] != "1" || !matches!(fields[1], "0" | "1") {
            return Err(err("invalid pane shell state"));
        }
        let executable = String::from_utf8(unhex(fields[2])?)?;
        if executable.is_empty() {
            return Err(err("invalid pane shell state"));
        }
        return Ok(Some(SavedShell {
            executable,
            login: fields[1] == "1",
        }));
    }
    let executable = process::tmux_quiet(
        config,
        &["show-options", "-pqv", "-t", pane, "@atelier_restart_shell"],
    )
    .unwrap_or_default();
    if executable.is_empty() {
        return Ok(None);
    }
    let login = process::tmux_quiet(
        config,
        &[
            "show-options",
            "-pqv",
            "-t",
            pane,
            "@atelier_restart_shell_login",
        ],
    )
    .unwrap_or_default();
    if !matches!(login.as_str(), "0" | "1") {
        return Err(err("invalid pane shell login state"));
    }
    Ok(Some(SavedShell {
        executable,
        login: login == "1",
    }))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn unhex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return Err(err("invalid pane shell state"));
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = (pair[0] as char)
                .to_digit(16)
                .ok_or_else(|| err("invalid pane shell state"))?;
            let low = (pair[1] as char)
                .to_digit(16)
                .ok_or_else(|| err("invalid pane shell state"))?;
            Ok(((high << 4) | low) as u8)
        })
        .collect()
}

pub fn poll_interval(config: &Config) -> Result<Option<u64>> {
    if !cfg!(target_os = "linux") {
        return Ok(None);
    }
    duration_option(config, "@atelier_restart_interval", "5")
}

fn minimum_runtime(config: &Config) -> Result<Duration> {
    duration_option(config, "@atelier_restart_min_runtime", "5")?
        .map(Duration::from_secs)
        .ok_or_else(|| err("@atelier_restart_min_runtime cannot be off"))
}

fn duration_option(config: &Config, option: &str, fallback: &str) -> Result<Option<u64>> {
    let value = process::tmux_quiet(config, &["show-options", "-gqv", option])
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.into());
    if value == "off" {
        return Ok(None);
    }
    let seconds: u64 = value.parse().map_err(|_| {
        err(format!(
            "invalid {option}: expected a positive number or off"
        ))
    })?;
    if seconds == 0 {
        return Err(err(format!(
            "invalid {option}: expected a positive number or off"
        )));
    }
    Ok(Some(seconds))
}

pub fn restart_disposition(
    config: &Config,
    shell: &SavedShell,
    process: &SavedProcess,
) -> Result<RestartDisposition> {
    let family = Path::new(&shell.executable)
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    if !matches!(family.as_ref(), "bash" | "zsh" | "fish") {
        return Ok(RestartDisposition::ShellOnly {
            command: shell_command(shell),
            warning: format!(
                "did not restart {} because shell {} is unsupported",
                process
                    .argv
                    .first()
                    .map(String::as_str)
                    .unwrap_or("process"),
                shell.executable
            ),
        });
    }
    let cli = process::tmux_quiet(config, &["show-options", "-gqv", "@atelier_cli"])
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("TMUX_ATELIER_CLI").ok())
        .ok_or_else(|| err("@atelier_cli is not configured"))?;
    let mut command = format!(
        "{} internal pane-run --shell {} --executable {} --debug-log {}",
        quote_sh(&cli),
        quote_sh(&shell.executable),
        quote_sh(&process.executable),
        quote_sh(&config.debug_log.to_string_lossy())
    );
    if shell.login {
        command.push_str(" --login");
    }
    command.push_str(" --");
    for argument in &process.argv {
        command.push(' ');
        command.push_str(&quote_sh(argument));
    }
    Ok(RestartDisposition::Runnable(command))
}

pub fn shell_command(shell: &SavedShell) -> String {
    let login = if shell.login { " -l" } else { "" };
    format!(
        "/bin/sh -c {}",
        quote_sh(&format!("exec {}{login}", quote_sh(&shell.executable)))
    )
}

fn read_entry(pid: i32) -> Result<ProcEntry> {
    let root = PathBuf::from(format!("/proc/{pid}"));
    let stat = fs::read_to_string(root.join("stat"))?;
    let close = stat.rfind(") ").ok_or_else(|| err("invalid /proc stat"))?;
    let fields: Vec<_> = stat[close + 2..].split_ascii_whitespace().collect();
    if fields.len() < 20 {
        return Err(err("invalid /proc stat"));
    }
    let argv = parse_cmdline(&fs::read(root.join("cmdline"))?)?;
    Ok(ProcEntry {
        ppid: fields[1].parse()?,
        pgrp: fields[2].parse()?,
        tty: fields[4].parse()?,
        tpgid: fields[5].parse()?,
        start_ticks: fields[19].parse()?,
        executable: fs::read_link(root.join("exe"))?
            .to_string_lossy()
            .into_owned(),
        argv,
    })
}

fn parse_cmdline(cmdline: &[u8]) -> Result<Vec<String>> {
    if cmdline.is_empty() {
        return Err(err("process has no argv"));
    }
    let bytes = cmdline.strip_suffix(&[0]).unwrap_or(cmdline);
    bytes
        .split(|byte| *byte == 0)
        .map(|arg| String::from_utf8(arg.to_vec()).map_err(Into::into))
        .collect()
}

fn process_runtime(start_ticks: u64) -> Result<Duration> {
    let uptime: f64 = fs::read_to_string("/proc/uptime")?
        .split_ascii_whitespace()
        .next()
        .ok_or_else(|| err("invalid /proc uptime"))?
        .parse()?;
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks <= 0 {
        return Err(err("could not read process clock rate"));
    }
    Ok(Duration::from_secs_f64(
        (uptime - start_ticks as f64 / ticks as f64).max(0.0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmdline_preserves_empty_arguments() {
        assert_eq!(
            parse_cmdline(b"program\0\0value\0\0").unwrap(),
            ["program", "", "value", ""]
        );
        assert_eq!(parse_cmdline(b"\0").unwrap(), [""]);
        assert_eq!(parse_cmdline(b"\0value\0").unwrap(), ["", "value"]);
        assert_eq!(
            parse_cmdline(b"program\0value").unwrap(),
            ["program", "value"]
        );
    }

    #[test]
    fn cmdline_rejects_empty_and_non_utf8_input() {
        assert!(parse_cmdline(b"").is_err());
        assert!(parse_cmdline(&[0xff, 0]).is_err());
    }

    #[test]
    fn denylist_applies_to_the_saved_root_executable() {
        assert!(!executable_is_denylisted("/usr/bin/opencode", "node npm"));
        assert!(executable_is_denylisted("/usr/bin/node", "node npm"));
    }
}
