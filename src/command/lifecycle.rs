use std::env;
use std::fs;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use super::{ui, App};
use crate::{err, process, wizard, workspace, Result};
use workspace::Workspace;

pub(super) fn new(app: &App, args: &[String], requested_shell: Option<&str>) -> Result<()> {
    let mut name = args.get(1).map(String::as_str).unwrap_or("");
    let mut detached = args.get(2).map(String::as_str).unwrap_or("");
    if name == "--detached" {
        detached = name;
        name = "";
    }
    if !detached.is_empty() && detached != "--detached" {
        return Err(err(format!("unknown argument: {detached}")));
    }
    let (destination, path) = workspace::parse_target(&args[0])?;
    let shell = requested_shell.unwrap_or(if destination == "local" {
        "local"
    } else {
        "posix"
    });
    let name = if name.is_empty() {
        workspace::normalise_name(app, &path, "", "")
    } else {
        name.into()
    };
    workspace::validate_name(&name)?;
    if app.workspaces.join(&name).exists() {
        return Err(err(format!("workspace already exists: {name}")));
    }
    if workspace::session_exists(app, &name) {
        return Err(err(format!("tmux session already exists: {name}")));
    }
    if destination == "local" && !Path::new(&path).is_dir() {
        return Err(err(format!("local path is not a directory: {path}")));
    }
    let definition = Workspace::new(&name, &destination, &path, Some(shell))?;
    workspace::write(app, &definition, true)
        .map_err(|_| err(format!("workspace already exists: {name}")))?;
    if workspace::create_session(app, &definition, &path).is_err() {
        return Err(err(format!(
            "definition saved, but the session could not be created: {name}"
        )));
    }
    app.snapshot("", "")?;
    app.refresh_status_if_running()?;
    select(app, &name, detached == "--detached")
}

pub(super) fn open(app: &App, name: &str, detached: Option<&str>) -> Result<()> {
    workspace::validate_name(name)?;
    if detached.is_some_and(|argument| argument != "--detached") {
        return Err(err(format!("unknown argument: {}", detached.unwrap())));
    }
    if workspace::session_exists(app, name) && app.workspaces.join(name).is_file() {
        let definition = workspace::read(app, name)?;
        let destination = workspace::session_option(app, name, "@atelier_destination");
        let path = workspace::session_option(app, name, "@atelier_path");
        let shell = super::shell_option(app, name, &destination);
        if workspace::session_option(app, name, "@atelier_managed") != "1"
            || destination != definition.destination
            || path != definition.path
            || shell != definition.shell
        {
            return Err(err(format!(
                "saved workspace conflicts with tmux session: {name}"
            )));
        }
    } else if !workspace::session_exists(app, name) {
        let definition = workspace::read(app, name)?;
        workspace::create_session(app, &definition, &definition.path)?;
    }
    app.snapshot("", "")?;
    app.refresh_status_if_running()?;
    select(app, name, detached == Some("--detached"))
}

fn select(app: &App, name: &str, detached: bool) -> Result<()> {
    if detached {
        return Ok(());
    }
    if env::var_os("TMUX").is_some() {
        process::tmux(app, &["switch-client", "-t", &format!("={name}")])
    } else {
        Err(Command::new(&app.tmux)
            .args(["attach-session", "-t", &format!("={name}")])
            .exec()
            .into())
    }
}

pub(super) fn rename(app: &App, old: &str, new: &str) -> Result<()> {
    workspace::validate_name(old)?;
    workspace::validate_name(new)?;
    if old == new {
        return Ok(());
    }
    let old_file = app.workspaces.join(old);
    if app.workspaces.join(new).exists() {
        return Err(err(format!("workspace already exists: {new}")));
    }
    if workspace::session_exists(app, new) {
        return Err(err(format!("tmux session already exists: {new}")));
    }
    if !old_file.is_file() && !workspace::session_exists(app, old) {
        return Err(err(format!("workspace not found: {old}")));
    }
    let definition = old_file
        .is_file()
        .then(|| workspace::read(app, old))
        .transpose()?;
    let mut renamed = false;
    if workspace::session_exists(app, old) {
        process::tmux(app, &["rename-session", "-t", &format!("={old}"), new])?;
        renamed = true;
    }
    if let Some(mut definition) = definition {
        definition.name = new.into();
        if workspace::write(app, &definition, true).is_err() {
            if renamed {
                let _ = process::tmux(app, &["rename-session", "-t", &format!("={new}"), old]);
            }
            return Err(err(format!("could not rename workspace: {old}")));
        }
        fs::remove_file(old_file)?;
    }
    app.snapshot("", "")?;
    app.refresh_status_if_running()
}

pub(super) fn edit(
    app: &App,
    name: &str,
    target: &str,
    requested_shell: Option<&str>,
) -> Result<()> {
    workspace::validate_name(name)?;
    let mut definition = workspace::read(app, name)?;
    let old_destination = definition.destination.clone();
    let old_shell = definition.shell.clone();
    let (destination, path) = workspace::parse_target(target)?;
    if destination == "local" && !Path::new(&path).is_dir() {
        return Err(err(format!("local path is not a directory: {path}")));
    }
    definition.shell = requested_shell
        .unwrap_or(if destination == "local" {
            "local"
        } else if destination == old_destination {
            &old_shell
        } else {
            "posix"
        })
        .into();
    definition.destination = destination;
    definition.path = path;
    workspace::write(app, &definition, false)
        .map_err(|_| err(format!("could not update workspace: {name}")))?;
    if workspace::session_exists(app, name) {
        workspace::mark_session(app, &definition).map_err(|_| {
            err(format!(
                "workspace saved, but the running session could not be updated: {name}"
            ))
        })?;
    }
    app.refresh_status_if_running()
}

pub(super) fn close(app: &App, name: &str) -> Result<()> {
    workspace::validate_name(name)?;
    if !workspace::session_exists(app, name) {
        return Err(err(format!("session is not running: {name}")));
    }
    let clients = process::tmux_quiet(
        app,
        &["list-clients", "-F", "#{session_name}\t#{client_name}"],
    )
    .unwrap_or_default();
    let clients: Vec<_> = clients
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .filter(|(session, client)| *session == name && !client.is_empty())
        .map(|(_, client)| client.to_owned())
        .collect();
    let fallback = if clients.is_empty() {
        String::new()
    } else {
        close_fallback(app, name)?
            .ok_or_else(|| err("cannot close the only workspace while a client is attached"))?
    };
    for client in clients {
        process::tmux(
            app,
            &[
                "switch-client",
                "-c",
                &client,
                "-t",
                &format!("={fallback}"),
            ],
        )
        .map_err(|_| {
            err(format!(
                "could not switch client away from workspace: {name}"
            ))
        })?;
    }
    app.snapshot(name, "")?;
    if process::tmux(app, &["kill-session", "-t", &format!("={name}")]).is_err() {
        app.snapshot("", "")?;
        return Err(err(format!("could not close workspace: {name}")));
    }
    app.refresh_status_if_running()
}

fn close_fallback(app: &App, excluded: &str) -> Result<Option<String>> {
    if let Some(name) = workspace::session_names(app)
        .into_iter()
        .find(|name| name != excluded)
    {
        return Ok(Some(name));
    }
    for name in workspace::definition_names(app)? {
        if name != excluded && open(app, &name, Some("--detached")).is_ok() {
            return Ok(Some(name));
        }
    }
    Ok(None)
}

pub(super) fn delete(app: &App, name: &str) -> Result<()> {
    let file = workspace::path(app, name)?;
    if !file.is_file() {
        return Err(err(format!("workspace definition not found: {name}")));
    }
    if workspace::session_exists(app, name) {
        close(app, name)?;
    }
    fs::remove_file(file)?;
    app.snapshot("", "")?;
    app.refresh_status_if_running()
}

pub(super) fn request_close(app: &App, name: &str, client: Option<&str>) -> Result<()> {
    workspace::validate_name(name)?;
    ui::popup_request(app, "confirm-close", name, client, "50%", "20%")
}

pub(super) fn request_delete(app: &App, name: &str, client: Option<&str>) -> Result<()> {
    workspace::validate_name(name)?;
    ui::popup_request(app, "confirm-delete", name, client, "50%", "20%")
}

pub(super) fn confirm_close(app: &App, name: &str) -> Result<()> {
    workspace::validate_name(name)?;
    if app.confirm(&format!("Stop workspace {name}?"))? == Some(true) {
        close(app, name)
    } else {
        Ok(())
    }
}

pub(super) fn confirm_delete(app: &App, name: &str) -> Result<()> {
    workspace::validate_name(name)?;
    if app.confirm(&format!("Delete workspace {name}?"))? == Some(true) {
        delete(app, name)
    } else {
        Ok(())
    }
}

pub(super) fn request_rename(app: &App, name: &str, client: Option<&str>) -> Result<()> {
    workspace::validate_name(name)?;
    ui::popup_request(app, "popup-rename", name, client, "50%", "20%")
}

pub(super) fn popup_rename(app: &App, old: &str) -> Result<()> {
    workspace::validate_name(old)?;
    let Some(new) = app.input("New workspace name", Some(old))? else {
        return Ok(());
    };
    rename(app, old, &new)
}

pub(super) fn popup_new(app: &App) -> Result<()> {
    let client = env::var("TMUX_ATELIER_CLIENT").unwrap_or_default();
    loop {
        let Some(target) = wizard::choose_target(app, app.interaction.as_ref())? else {
            return Ok(());
        };
        let suggested = workspace::normalise_name(app, &target.path, "", "");
        let Some(name) = app.input("Workspace name", Some(&suggested))? else {
            continue;
        };
        let args = [
            format!("{}:{}", target.destination, target.path),
            name.clone(),
            "--detached".into(),
        ];
        new(app, &args, Some(&target.shell))?;
        if !client.is_empty() {
            process::tmux(
                app,
                &["switch-client", "-c", &client, "-t", &format!("={name}")],
            )?;
        } else {
            select(app, &name, false)?;
        }
        return Ok(());
    }
}

pub(super) fn popup_edit(app: &App, name: &str) -> Result<()> {
    workspace::validate_name(name)?;
    workspace::read(app, name)?;
    let Some(target) = wizard::choose_target(app, app.interaction.as_ref())? else {
        return Ok(());
    };
    let suggested = workspace::normalise_name(app, &target.path, name, name);
    app.debug(&format!(
        "workspace-edit name-prompt name={name} suggested={suggested}"
    ))?;
    let Some(new) = app.input("Workspace name", Some(&suggested))? else {
        return Ok(());
    };
    rename(app, name, &new)?;
    edit(
        app,
        &new,
        &format!("{}:{}", target.destination, target.path),
        Some(&target.shell),
    )
}
