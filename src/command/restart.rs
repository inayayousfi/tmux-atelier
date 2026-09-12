use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Child, Command, ExitStatus};
use std::time::{SystemTime, UNIX_EPOCH};

use super::App;
use crate::config::quote_sh;
use crate::process_state::{self, RestartPolicy};
use crate::{Result, err, process};

pub(super) fn set(app: &App, policy: RestartPolicy, pane: Option<&str>) -> Result<()> {
    let pane = match pane {
        Some(pane) => pane.into(),
        None => process::tmux_output(app, &["display-message", "-p", "#{pane_id}"])?,
    };
    validate_pane(&pane)?;
    process_state::set_pane_policy(app, &pane, policy)?;
    app.snapshot("", "")
}

pub(super) fn arm(app: &App) -> Result<()> {
    let generation = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    app.set_global("@atelier_restart_poll_generation", &generation)?;
    schedule(app, &generation)
}

pub(super) fn poll(app: &App, generation: &str) -> Result<()> {
    let current = process::tmux_quiet(
        app,
        &["show-options", "-gqv", "@atelier_restart_poll_generation"],
    )
    .unwrap_or_default();
    if current != generation {
        return Ok(());
    }
    let snapshot = app.snapshot("", "");
    let successor = schedule(app, generation);
    match (snapshot, successor) {
        (Err(error), _) => Err(error),
        (Ok(()), result) => result,
    }
}

pub(super) fn pane_run(
    debug_log: &Path,
    shell: &str,
    login: bool,
    executable: &str,
    argv: &[String],
) -> Result<()> {
    if argv.is_empty() {
        return Err(err("pane runner requires a command"));
    }
    let program = &argv[0];
    let _ = crate::config::debug_to(
        debug_log,
        &format!(
            "process launcher starting program={} executable={} arguments={} shell={} login={login}",
            crate::config::shell_debug(program),
            crate::config::shell_debug(executable),
            argv.len(),
            crate::config::shell_debug(shell),
        ),
    );
    let cli = std::env::current_exe()?;
    let mut child = Command::new(cli);
    child.args([
        "internal",
        "process-guard",
        "--shell",
        shell,
        "--executable",
        executable,
    ]);
    if login {
        child.arg("--login");
    }
    let mut child = match child.arg("--").args(argv).process_group(0).spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = crate::config::debug_to(
                debug_log,
                &format!(
                    "process launcher spawn failed program={} error={error}",
                    crate::config::shell_debug(program)
                ),
            );
            return Err(error.into());
        }
    };
    let _ = crate::config::debug_to(
        debug_log,
        &format!(
            "process launcher spawned program={} child_pid={}",
            crate::config::shell_debug(program),
            child.id()
        ),
    );
    let status = run_foreground_child(debug_log, program, &mut child)?;
    let outcome = if let Some(signal) = status.signal() {
        format!("signal={signal}")
    } else {
        format!("status={}", status.code().unwrap_or(1))
    };
    let _ = crate::config::debug_to(
        debug_log,
        &format!(
            "process launcher exited program={} child_pid={} {outcome}",
            crate::config::shell_debug(program),
            child.id()
        ),
    );
    if !status.success() {
        if let Some(signal) = status.signal() {
            eprintln!(
                "tmux-atelier: {} stopped by signal {signal}",
                argv.first().unwrap()
            );
        } else {
            eprintln!(
                "tmux-atelier: {} exited with status {}",
                argv.first().unwrap(),
                status.code().unwrap_or(1)
            );
        }
    }
    let mut restored_shell = Command::new(shell);
    if login {
        restored_shell.arg("-l");
    }
    let _ = crate::config::debug_to(
        debug_log,
        &format!(
            "process launcher returning to shell program={} shell={} login={login}",
            crate::config::shell_debug(program),
            crate::config::shell_debug(shell)
        ),
    );
    let error = restored_shell.exec();
    let _ = crate::config::debug_to(
        debug_log,
        &format!(
            "process launcher shell fallback failed program={} error={error}",
            crate::config::shell_debug(program)
        ),
    );
    Err(error.into())
}

fn run_foreground_child(debug_log: &Path, program: &str, child: &mut Child) -> Result<ExitStatus> {
    let pid = child.id() as libc::pid_t;
    let status = match wait_for_child(pid) {
        Ok(status) => status,
        Err(error) => {
            let _ = terminate_process_group(child);
            return Err(error.into());
        }
    };
    if !libc::WIFSTOPPED(status) {
        return Ok(ExitStatus::from_raw(status));
    }
    let _ = crate::config::debug_to(
        debug_log,
        &format!(
            "process launcher child stopped program={} child_pid={}",
            crate::config::shell_debug(program),
            child.id()
        ),
    );

    let parent_pgid = unsafe { libc::getpgrp() };
    if let Err(error) = set_foreground_process_group(pid) {
        let _ = terminate_process_group(child);
        log_launcher_error(debug_log, program, child.id(), "terminal handoff", &error);
        return Err(error);
    }
    if unsafe { libc::kill(-pid, libc::SIGCONT) } != 0 {
        let error: crate::Error = std::io::Error::last_os_error().into();
        let _ = terminate_process_group(child);
        let reclaim = set_foreground_process_group(parent_pgid);
        log_launcher_error(debug_log, program, child.id(), "child resume", &error);
        if let Err(reclaim) = reclaim {
            log_launcher_error(debug_log, program, child.id(), "terminal reclaim", &reclaim);
        }
        return Err(error);
    }
    let _ = crate::config::debug_to(
        debug_log,
        &format!(
            "process launcher terminal handed off program={} child_pid={}",
            crate::config::shell_debug(program),
            child.id()
        ),
    );

    let status = wait_for_child(pid);
    let reclaim = set_foreground_process_group(parent_pgid);
    let status = match status {
        Ok(status) if libc::WIFSTOPPED(status) => {
            let signal = libc::WSTOPSIG(status);
            let _ = crate::config::debug_to(
                debug_log,
                &format!(
                    "process launcher canceled stopped child program={} child_pid={} signal={signal}",
                    crate::config::shell_debug(program),
                    child.id()
                ),
            );
            terminate_process_group(child)
        }
        Ok(status) => Ok(ExitStatus::from_raw(status)),
        Err(error) => {
            let _ = terminate_process_group(child);
            Err(error)
        }
    };
    match (status, reclaim) {
        (Ok(status), Ok(())) => Ok(status),
        (Err(error), reclaim) => {
            let error: crate::Error = error.into();
            log_launcher_error(debug_log, program, child.id(), "wait", &error);
            if let Err(reclaim) = reclaim {
                log_launcher_error(debug_log, program, child.id(), "terminal reclaim", &reclaim);
            }
            Err(error)
        }
        (Ok(_), Err(error)) => {
            log_launcher_error(debug_log, program, child.id(), "terminal reclaim", &error);
            Err(error)
        }
    }
}

fn wait_for_child(pid: libc::pid_t) -> std::io::Result<i32> {
    loop {
        let mut status = 0;
        let result = unsafe { libc::waitpid(pid, &mut status, libc::WUNTRACED) };
        if result == pid {
            return Ok(status);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn set_foreground_process_group(pgid: libc::pid_t) -> Result<()> {
    let previous = unsafe { libc::signal(libc::SIGTTOU, libc::SIG_IGN) };
    if previous == libc::SIG_ERR {
        return Err(std::io::Error::last_os_error().into());
    }
    let result = unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, pgid) };
    let error = (result != 0).then(std::io::Error::last_os_error);
    if unsafe { libc::signal(libc::SIGTTOU, previous) } == libc::SIG_ERR {
        return Err(std::io::Error::last_os_error().into());
    }
    match error {
        Some(error) => Err(error.into()),
        None => Ok(()),
    }
}

fn terminate_process_group(child: &mut Child) -> std::io::Result<ExitStatus> {
    if unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    child.wait()
}

fn log_launcher_error(
    debug_log: &Path,
    program: &str,
    child_pid: u32,
    phase: &str,
    error: &crate::Error,
) {
    let _ = crate::config::debug_to(
        debug_log,
        &format!(
            "process launcher {phase} failed program={} child_pid={child_pid} error={error}",
            crate::config::shell_debug(program)
        ),
    );
}

pub(super) fn process_guard(
    shell: &str,
    login: bool,
    executable: &str,
    argv: &[String],
) -> Result<()> {
    if argv.is_empty() {
        return Err(err("process guard requires argv"));
    }
    if unsafe { libc::raise(libc::SIGSTOP) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let cli = std::env::current_exe()?;
    let mut command = format!(
        "{} internal process-exec --executable {} --",
        quote_sh(&cli.to_string_lossy()),
        quote_sh(executable)
    );
    for argument in argv {
        command.push(' ');
        command.push_str(&quote_sh(argument));
    }
    let mut child = Command::new(shell);
    if login {
        child.arg("-l");
    }
    let error = child.args(["-i", "-c", &format!("exec {command}")]).exec();
    Err(error.into())
}

pub(super) fn process_exec(executable: &str, argv: &[String]) -> Result<()> {
    let Some(argv0) = argv.first() else {
        return Err(err("process launcher requires argv"));
    };
    let mut command = Command::new(executable);
    command.arg0(argv0).args(&argv[1..]);
    Err(command.exec().into())
}

fn schedule(app: &App, generation: &str) -> Result<()> {
    let Some(interval) = process_state::poll_interval(app)? else {
        return Ok(());
    };
    let command = format!(
        "{} internal poll-processes {}",
        quote_sh(&app.cli_path()?),
        quote_sh(generation)
    );
    process::tmux(
        app,
        &["run-shell", "-b", "-d", &interval.to_string(), &command],
    )
}

fn validate_pane(pane: &str) -> Result<()> {
    if pane.strip_prefix('%').is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        Ok(())
    } else {
        Err(err(format!("invalid tmux pane id: {pane}")))
    }
}
