use std::fs;
use std::io::{self, Read, Write};
use std::thread;
use std::time::Duration;

use super::{ui, App};
use crate::config::quote_sh;
use crate::{err, process, snapshot, workspace, Result};
use workspace::Workspace;

pub(super) fn adopt(app: &App, session: &str, client: Option<&str>) -> Result<()> {
    snapshot::lock(app, &app.adoption_lock, || {
        adopt_inner(app, session, client)
    })
}

fn adopt_inner(app: &App, session: &str, client: Option<&str>) -> Result<()> {
    if session.is_empty()
        || !workspace::session_exists(app, session)
        || workspace::session_option(app, session, "@atelier_managed") == "1"
    {
        return Ok(());
    }
    if process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore_pending"]).as_deref()
        == Some("1")
    {
        app.debug(&format!(
            "session adoption deferred session={session} restore=pending"
        ))?;
        return Ok(());
    }
    let raw_path = process::tmux_output(
        app,
        &[
            "display-message",
            "-p",
            "-t",
            &format!("={session}:"),
            "#{pane_current_path}",
        ],
    )?;
    let path = match workspace::canonical_local_path(&raw_path) {
        Ok(path) => path,
        Err(_) => {
            app.debug(&format!(
                "session adoption skipped session={session} reason=invalid-cwd"
            ))?;
            return Ok(());
        }
    };
    let inferred_client = if client.unwrap_or_default().is_empty() {
        client_for_session(app, session)
    } else {
        client.map(str::to_owned)
    };
    let existing = workspace::workspace_for_local_path(app, &path)?;
    if let Some(existing) = existing
        .as_deref()
        .filter(|existing| *existing != session && workspace::session_exists(app, existing))
    {
        if let Some(client) = inferred_client.as_deref().filter(|value| !value.is_empty()) {
            process::tmux(
                app,
                &["switch-client", "-c", client, "-t", &format!("={existing}")],
            )?;
        }
        process::tmux(app, &["kill-session", "-t", &format!("={session}")])?;
        app.snapshot("", "")?;
        return app.refresh_status_if_running();
    }
    let name = existing
        .clone()
        .unwrap_or_else(|| workspace::normalise_name(app, &path, session, ""));
    let created_definition = existing.is_none();
    if created_definition {
        workspace::write(
            app,
            &Workspace::new(&name, "local", &path, Some("local"))?,
            true,
        )
        .map_err(|_| err(format!("could not save adopted workspace: {name}")))?;
    }
    let old = session.to_owned();
    let mut current = old.clone();
    if current != name {
        if process::tmux(
            app,
            &["rename-session", "-t", &format!("={current}"), &name],
        )
        .is_err()
        {
            if created_definition {
                let _ = fs::remove_file(app.workspaces.join(&name));
            }
            return Err(err("could not rename adopted session"));
        }
        current = name.clone();
    }
    let definition = workspace::read(app, &name)?;
    if workspace::mark_session(app, &definition).is_err() {
        if current != old {
            let _ = process::tmux(app, &["rename-session", "-t", &format!("={current}"), &old]);
        }
        if created_definition {
            let _ = fs::remove_file(app.workspaces.join(&name));
        }
        return Err(err("could not mark adopted session"));
    }
    app.debug(&format!(
        "session adopted old={old} workspace={name} path={}",
        crate::config::shell_debug(&path)
    ))?;
    app.snapshot("", "")?;
    app.refresh_status_if_running()
}

fn client_for_session(app: &App, wanted: &str) -> Option<String> {
    process::tmux_quiet(
        app,
        &["list-clients", "-F", "#{session_name}\t#{client_name}"],
    )
    .and_then(|clients| {
        clients
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .find(|(session, _)| *session == wanted)
            .map(|(_, client)| client.into())
    })
}

pub(super) fn discard(app: &App, client: Option<&str>) -> Result<()> {
    app.debug("restore discarded; starting fresh")?;
    match fs::remove_file(&app.restore_file) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    app.set_global("@atelier_restore_pending", "0")?;
    app.set_global("@atelier_restore_handled", "1")?;
    app.set_global("@atelier_restore_started", "1")?;
    app.snapshot("", "")?;
    if let Some(client) = client.filter(|value| !value.is_empty()) {
        if let Some(session) = process::tmux_quiet(
            app,
            &["display-message", "-p", "-c", client, "#{session_name}"],
        )
        .filter(|value| !value.is_empty())
        {
            adopt(app, &session, Some(client))?;
        }
    }
    Ok(())
}

pub(super) fn run(app: &App, client: Option<&str>) -> Result<()> {
    if !app.restore_file.is_file() {
        return Ok(());
    }
    let saved = match snapshot::read(app) {
        Ok(saved) => saved,
        Err(_) => {
            app.debug("restore rejected invalid snapshot")?;
            discard(app, client)?;
            return Err(err("invalid restore snapshot"));
        }
    };
    let bootstrap = client
        .and_then(|client| {
            process::tmux_quiet(
                app,
                &["display-message", "-p", "-c", client, "#{session_name}"],
            )
        })
        .unwrap_or_default();
    app.debug(&format!(
        "restore started client={} active={}",
        client.unwrap_or_default(),
        saved.active
    ))?;
    app.set_global("@atelier_restore_pending", "1")?;
    app.set_global("@atelier_restore_handled", "0")?;
    if snapshot::restore(app, &saved).is_err() {
        app.set_global("@atelier_restore_pending", "1")?;
        app.set_global("@atelier_restore_handled", "0")?;
        app.set_global("@atelier_restore_started", "0")?;
        app.debug("restore failed; created sessions rolled back and retry enabled")?;
        return Err(err("workspace restoration failed"));
    }
    app.set_global("@atelier_restore_pending", "0")?;
    app.set_global("@atelier_restore_handled", "1")?;
    app.set_global("@atelier_restore_started", "1")?;
    app.snapshot("", "")?;
    ui::refresh_status(app)?;
    if !bootstrap.is_empty()
        && bootstrap != saved.active
        && workspace::session_exists(app, &bootstrap)
        && workspace::session_option(app, &bootstrap, "@atelier_managed") != "1"
    {
        process::tmux(app, &["kill-session", "-t", &format!("={bootstrap}")])?;
    }
    if let Some(client) =
        client.filter(|_| !saved.active.is_empty() && workspace::session_exists(app, &saved.active))
    {
        process::tmux(
            app,
            &[
                "switch-client",
                "-c",
                client,
                "-t",
                &format!("={}", saved.active),
            ],
        )?;
    }
    app.debug(&format!(
        "restore completed client={} active={}",
        client.unwrap_or_default(),
        saved.active
    ))
}

pub(super) fn popup_restore(app: &App, client: Option<&str>) -> Result<()> {
    let saved = snapshot::read(app).map_err(|_| err("invalid restore snapshot"))?;
    let (workspaces, windows, panes) = saved.counts();
    print!("Restore {workspaces} workspaces, {windows} tabs, and {panes} panes? [y/N] ");
    io::stdout().flush()?;
    let mut byte = [0];
    let accepted = io::stdin().read_exact(&mut byte).is_ok() && matches!(byte[0], b'y' | b'Y');
    println!();
    if accepted {
        run(app, client)
    } else {
        println!("Starting fresh.");
        discard(app, client)
    }
}

pub(super) fn arm(app: &App) -> Result<()> {
    let handled = process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore_handled"])
        .unwrap_or_default();
    if !handled.is_empty() {
        app.debug(&format!("restore arm skipped handled={handled}"))?;
        return Ok(());
    }
    for session in workspace::session_names(app) {
        if workspace::session_option(app, &session, "@atelier_managed") == "1" {
            app.set_global("@atelier_restore_handled", "1")?;
            app.set_global("@atelier_restore_pending", "0")?;
            app.set_global("@atelier_restore_started", "1")?;
            app.snapshot("", "")?;
            return Ok(());
        }
    }
    if app.restore_file.is_file() {
        app.set_global("@atelier_restore_handled", "0")?;
        app.set_global("@atelier_restore_pending", "1")?;
        app.set_global("@atelier_restore_started", "0")?;
    } else {
        app.set_global("@atelier_restore_handled", "1")?;
        app.set_global("@atelier_restore_pending", "0")?;
        app.set_global("@atelier_restore_started", "1")?;
    }
    Ok(())
}

pub(super) fn start(app: &App, client: Option<&str>) -> Result<()> {
    let handled = process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore_handled"])
        .unwrap_or_default();
    if handled != "0" {
        return Ok(());
    }
    if !app.restore_file.is_file() {
        return discard(app, client);
    }
    let mode = process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore"])
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "prompt".into());
    if !matches!(mode.as_str(), "always" | "never" | "prompt") {
        return Err(err(
            "invalid @atelier_restore value: expected always, never, or prompt",
        ));
    }
    if mode == "prompt" && client.unwrap_or_default().is_empty() {
        return Ok(());
    }
    process::tmux(app, &["wait-for", "-L", "atelier-restore-start"])?;
    let handled = process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore_handled"])
        .unwrap_or_default();
    let started = process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore_started"])
        .unwrap_or_default();
    if handled != "0" || started == "1" {
        process::tmux(app, &["wait-for", "-U", "atelier-restore-start"])?;
        return Ok(());
    }
    if app.set_global("@atelier_restore_started", "1").is_err() {
        let _ = process::tmux(app, &["wait-for", "-U", "atelier-restore-start"]);
        return Err(err("could not start restore"));
    }
    process::tmux(app, &["wait-for", "-U", "atelier-restore-start"])?;
    match mode.as_str() {
        "always" => run(app, client),
        "never" => discard(app, client),
        "prompt" => {
            let client = client.unwrap();
            name_bootstrap(app, client)?;
            let command = format!(
                "{} popup-restore {}",
                quote_sh(&app.cli_path()?),
                quote_sh(client)
            );
            if process::tmux(
                app,
                &[
                    "display-popup",
                    "-c",
                    client,
                    "-E",
                    "-w",
                    "55%",
                    "-h",
                    "20%",
                    &command,
                ],
            )
            .is_err()
            {
                app.set_global("@atelier_restore_started", "0")?;
                return Err(err("could not open restore prompt"));
            }
            let handled =
                process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore_handled"])
                    .unwrap_or_default();
            if handled == "0" {
                app.set_global("@atelier_restore_started", "0")?;
            }
            Ok(())
        }
        _ => unreachable!(),
    }
}

fn name_bootstrap(app: &App, client: &str) -> Result<()> {
    let Some(session) = process::tmux_quiet(
        app,
        &["display-message", "-p", "-c", client, "#{session_name}"],
    ) else {
        return Ok(());
    };
    if session.is_empty()
        || workspace::session_option(app, &session, "@atelier_managed") == "1"
        || session == "restore-prompt"
        || session
            .strip_prefix("restore-prompt-")
            .is_some_and(|suffix| suffix.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Ok(());
    }
    let mut name = "restore-prompt".to_owned();
    let mut suffix = 2;
    while workspace::session_exists(app, &name) {
        name = format!("restore-prompt-{suffix}");
        suffix += 1;
    }
    let _ = process::tmux(
        app,
        &["rename-session", "-t", &format!("={session}"), &name],
    );
    Ok(())
}

pub(super) fn attached(app: &App) -> Result<()> {
    for _ in 0..100 {
        if let Some(client) = process::tmux_quiet(app, &["list-clients", "-F", "#{client_name}"])
            .and_then(|clients| clients.lines().next().map(str::to_owned))
            .filter(|client| !client.is_empty())
        {
            return start(app, Some(&client));
        }
        thread::sleep(Duration::from_millis(10));
    }
    app.debug("restore attached-client check found no client")
}
