use super::{ui, App};
use crate::config::quote_sh;
use crate::{err, process, workspace, Result};

pub(super) fn window(app: &App, session: Option<&str>) -> Result<()> {
    let session = match session {
        Some(value) => value.into(),
        None => process::tmux_output(app, &["display-message", "-p", "#{session_name}"])?,
    };
    let target = format!("={session}:{{end}}");
    if workspace::session_option(app, &session, "@atelier_managed") != "1" {
        return process::tmux(app, &["new-window", "-a", "-t", &target]);
    }
    let destination = workspace::session_option(app, &session, "@atelier_destination");
    let path = workspace::session_option(app, &session, "@atelier_path");
    let shell = super::shell_option(app, &session, &destination);
    if destination == "local" {
        process::tmux(app, &["new-window", "-a", "-t", &target, "-c", &path])?;
    } else {
        let command = process::remote_shell_command(app, &destination, &path, &shell)?;
        process::tmux(app, &["new-window", "-a", "-t", &target, &command])?;
    }
    app.snapshot("", "")
}

pub(super) fn split(app: &App, orientation: &str, pane: Option<&str>) -> Result<()> {
    if !matches!(orientation, "vertical" | "horizontal") {
        return Err(err("usage: tmux-atelier split vertical|horizontal [pane]"));
    }
    let pane = match pane {
        Some(value) => value.into(),
        None => process::tmux_output(app, &["display-message", "-p", "#{pane_id}"])?,
    };
    let session = process::tmux_output(
        app,
        &["display-message", "-p", "-t", &pane, "#{session_name}"],
    )?;
    let mut owned = vec!["split-window".to_owned(), "-t".into(), pane];
    if orientation == "vertical" {
        owned.push("-h".into());
    }
    let managed = workspace::session_option(app, &session, "@atelier_managed") == "1";
    if managed {
        let destination = workspace::session_option(app, &session, "@atelier_destination");
        let path = workspace::session_option(app, &session, "@atelier_path");
        if destination == "local" {
            owned.extend(["-c".into(), path]);
        } else {
            let shell = super::shell_option(app, &session, &destination);
            owned.push(process::remote_shell_command(
                app,
                &destination,
                &path,
                &shell,
            )?);
        }
    }
    process::tmux(app, &owned.iter().map(String::as_str).collect::<Vec<_>>())?;
    if managed {
        app.snapshot("", "")
    } else {
        Ok(())
    }
}

pub(super) fn request_rename(app: &App, window: &str, client: Option<&str>) -> Result<()> {
    validate_window(window)?;
    ui::popup_request(app, "popup-tab-rename", window, client, "50%", "20%")
}

pub(super) fn popup_rename(app: &App, window: &str) -> Result<()> {
    validate_window(window)?;
    let name = process::tmux_output(
        app,
        &["display-message", "-p", "-t", window, "#{window_name}"],
    )?;
    let Some(new) = process::read_line("New tab name: ", Some(&name))? else {
        return Ok(());
    };
    workspace::validate_value("tab name", &new)?;
    process::tmux(app, &["rename-window", "-t", window, &new])?;
    app.snapshot("", "")
}

pub(super) fn request_close(app: &App, window: &str, client: Option<&str>) -> Result<()> {
    validate_window(window)?;
    let name = process::tmux_output(
        app,
        &["display-message", "-p", "-t", window, "#{window_name}"],
    )?;
    let shell = format!(
        "{} tab-close {}",
        quote_sh(&app.cli_path()?),
        quote_sh(window)
    );
    ui::confirm_request(app, &format!("Close tab {name}? (y/n)"), &shell, client)
}

pub(super) fn close(app: &App, window: &str) -> Result<()> {
    validate_window(window)?;
    let session = process::tmux_output(
        app,
        &["display-message", "-p", "-t", window, "#{session_name}"],
    )?;
    let count: usize = process::tmux_output(
        app,
        &["display-message", "-p", "-t", window, "#{session_windows}"],
    )?
    .parse()?;
    if count == 1 {
        app.snapshot(&session, "")?;
    } else {
        app.snapshot("", window)?;
    }
    if process::tmux(app, &["kill-window", "-t", window]).is_err() {
        app.snapshot("", "")?;
        return Err(err("could not close tab"));
    }
    app.refresh_status_if_running()
}

pub(super) fn validate_window(window: &str) -> Result<()> {
    if window.strip_prefix('@').is_some_and(|number| {
        !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
    }) {
        Ok(())
    } else {
        Err(err(format!("invalid tmux window id: {window}")))
    }
}
