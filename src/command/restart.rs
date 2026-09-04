use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::Command;
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
    let program = argv.first().map(String::as_str).unwrap_or(executable);
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
    let mut child = match child
        .args(["-i", "-c", &format!("exec {command}")])
        .process_group(0)
        .spawn()
    {
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
    unsafe {
        libc::signal(libc::SIGTTOU, libc::SIG_IGN);
    }
    if unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, child.id() as libc::pid_t) } != 0 {
        let error = std::io::Error::last_os_error();
        let _ = crate::config::debug_to(
            debug_log,
            &format!(
                "process launcher terminal handoff failed program={} child_pid={} error={error}",
                crate::config::shell_debug(program),
                child.id()
            ),
        );
        return Err(error.into());
    }
    let _ = crate::config::debug_to(
        debug_log,
        &format!(
            "process launcher terminal handed off program={} child_pid={}",
            crate::config::shell_debug(program),
            child.id()
        ),
    );
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => {
            let _ = crate::config::debug_to(
                debug_log,
                &format!(
                    "process launcher wait failed program={} child_pid={} error={error}",
                    crate::config::shell_debug(program),
                    child.id()
                ),
            );
            return Err(error.into());
        }
    };
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
    if unsafe { libc::tcsetpgrp(libc::STDIN_FILENO, libc::getpgrp()) } != 0 {
        let error = std::io::Error::last_os_error();
        let _ = crate::config::debug_to(
            debug_log,
            &format!(
                "process launcher terminal reclaim failed program={} error={error}",
                crate::config::shell_debug(program)
            ),
        );
        return Err(error.into());
    }
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
