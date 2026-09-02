use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use super::{App, ui};
use crate::config::quote_sh;
use crate::process_state::ObservedForeground;
use crate::{Result, err, process, snapshot, workspace};
use workspace::Workspace;

pub(super) fn adopt(app: &App, session: &str, client: Option<&str>) -> Result<()> {
    let changed = snapshot::lock(app, &app.adoption_lock, || {
        adopt_inner(app, session, client)
    })?;
    if changed {
        app.snapshot("", "")?;
        app.refresh_status_if_running()?;
    }
    Ok(())
}

fn adopt_inner(app: &App, session: &str, client: Option<&str>) -> Result<bool> {
    if session.is_empty()
        || !workspace::session_exists(app, session)
        || workspace::session_option(app, session, "@atelier_managed") == "1"
    {
        return Ok(false);
    }
    if process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore_pending"]).as_deref()
        == Some("1")
    {
        app.debug(&format!(
            "session adoption deferred session={session} restore=pending"
        ))?;
        return Ok(false);
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
            return Ok(false);
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
        return Ok(true);
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
    Ok(true)
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
    if let Some(client) = client.filter(|value| !value.is_empty())
        && let Some(session) = process::tmux_quiet(
            app,
            &["display-message", "-p", "-c", client, "#{session_name}"],
        )
        .filter(|value| !value.is_empty())
    {
        adopt(app, &session, Some(client))?;
    }
    Ok(())
}

pub(super) fn run(
    app: &App,
    client: Option<&str>,
    approved_replacements: &HashMap<String, snapshot::WorkspaceTopology>,
    approved_processes: &HashMap<String, ObservedForeground>,
) -> Result<()> {
    snapshot::lock(app, &app.restore_lock, || {
        snapshot::recover_replacements(app)?;
        run_locked(app, client, approved_replacements, approved_processes)
    })
}

fn run_locked(
    app: &App,
    client: Option<&str>,
    approved_replacements: &HashMap<String, snapshot::WorkspaceTopology>,
    approved_processes: &HashMap<String, ObservedForeground>,
) -> Result<()> {
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
    let process_plan = match snapshot::plan_processes(app, &saved, approved_processes) {
        Ok(plan) => plan,
        Err(error) => {
            app.set_global("@atelier_restore_started", "0")?;
            return Err(error);
        }
    };
    let restored = snapshot::restore(app, &saved, approved_replacements);
    match restored {
        Ok(()) => {}
        Err(error) => {
            app.set_global("@atelier_restore_pending", "1")?;
            app.set_global("@atelier_restore_handled", "0")?;
            app.set_global("@atelier_restore_started", "0")?;
            app.debug(&format!(
                "restore failed; retry enabled reason=topology: {error}"
            ))?;
            return Err(err(format!(
                "workspace restoration failed: topology: {error}"
            )));
        }
    }
    let precommit = (|| -> Result<()> {
        for workspace in saved
            .workspaces
            .iter()
            .filter(|workspace| app.workspaces.join(&workspace.name).is_file())
        {
            if !snapshot::workspace_matches(app, workspace)? {
                return Err(err("restored topology did not match snapshot"));
            }
        }
        if let Some(client) = client
            .filter(|_| !saved.active.is_empty() && workspace::session_exists(app, &saved.active))
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
        Ok(())
    })();
    if let Err(error) = precommit {
        snapshot::recover_replacements(app)?;
        app.set_global("@atelier_restore_pending", "1")?;
        app.set_global("@atelier_restore_handled", "0")?;
        app.set_global("@atelier_restore_started", "0")?;
        app.debug(&format!("restore failed; retry enabled reason={error}"))?;
        return Err(err(format!("workspace restoration failed: {error}")));
    }
    let mut warnings = match snapshot::commit_replacements(app) {
        Ok(warnings) => warnings,
        Err(error) => {
            snapshot::recover_replacements(app)?;
            app.set_global("@atelier_restore_pending", "1")?;
            app.set_global("@atelier_restore_handled", "0")?;
            app.set_global("@atelier_restore_started", "0")?;
            return Err(error);
        }
    };
    warnings.extend(snapshot::apply_processes(app, process_plan));
    if !bootstrap.is_empty()
        && bootstrap != saved.active
        && workspace::session_exists(app, &bootstrap)
        && workspace::session_option(app, &bootstrap, "@atelier_managed") != "1"
        && let Err(error) = process::tmux(app, &["kill-session", "-t", &format!("={bootstrap}")])
    {
        warnings.push(format!("could not remove bootstrap session: {error}"));
    }
    let mut state_normalized = true;
    for (option, value) in [
        ("@atelier_restore_pending", "0"),
        ("@atelier_restore_handled", "1"),
        ("@atelier_restore_started", "1"),
    ] {
        if let Err(error) = app.set_global(option, value) {
            state_normalized = false;
            warnings.push(format!("could not set {option}: {error}"));
        }
    }
    warnings.extend(snapshot::finish_replacements(app, state_normalized));
    if let Err(error) = ui::refresh_status(app) {
        warnings.push(format!("could not refresh status: {error}"));
    }
    for warning in warnings {
        eprintln!("tmux-atelier: {warning}");
        if let Some(client) = client.filter(|client| !client.is_empty()) {
            let _ = process::tmux(app, &["display-message", "-c", client, &warning]);
        }
        let _ = app.debug(&format!("restore warning={warning}"));
    }
    let _ = app.debug(&format!(
        "restore completed client={} active={}",
        client.unwrap_or_default(),
        saved.active
    ));
    Ok(())
}

pub(super) fn popup_restore(app: &App, client: Option<&str>) -> Result<()> {
    let saved = snapshot::read(app).map_err(|_| err("invalid restore snapshot"))?;
    let changes = restore_changes(app, &saved)?;
    let mode = process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore"])
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "prompt".into());
    let (workspaces, windows, panes) = saved.counts();
    let process_count = changes.processes.len();
    if mode != "always"
        && !confirm_line(&format!(
            "Restore {workspaces} workspaces, {windows} tabs, {panes} panes, and {process_count} processes?"
        ))?
    {
        println!("Starting fresh.");
        return discard(app, client);
    }
    if !changes.processes.is_empty() {
        println!("Processes to restart:");
        for process in &changes.processes {
            println!(
                "  {} in {}:{}.{}",
                process.program, process.workspace, process.window, process.pane
            );
        }
    }
    let destructive_processes: Vec<_> = changes
        .processes
        .iter()
        .filter(|process| process.destructive())
        .collect();
    if !changes.mismatched.is_empty() || !destructive_processes.is_empty() {
        println!("The following live state will be replaced:");
        for name in changes.mismatched.keys() {
            println!("  workspace {name}");
        }
        for process in destructive_processes {
            println!(
                "  {} in {}:{}.{}",
                process.current.as_deref().unwrap_or("process"),
                process.workspace,
                process.window,
                process.pane
            );
        }
        if !confirm_line("Replace them and stop their current processes?")? {
            println!("Starting fresh.");
            return discard(app, client);
        }
    }
    let approved_workspaces = changes.mismatched;
    let approved_processes = changes
        .processes
        .into_iter()
        .filter(|process| process.destructive())
        .map(|process| (process.key, process.observed))
        .collect();
    run(app, client, &approved_workspaces, &approved_processes)
}

pub(super) fn arm(app: &App) -> Result<()> {
    snapshot::lock(app, &app.restore_lock, || {
        snapshot::recover_replacements(app)
    })?;
    let handled = process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore_handled"])
        .unwrap_or_default();
    if !handled.is_empty() {
        app.debug(&format!("restore arm skipped handled={handled}"))?;
        return Ok(());
    }
    if app.restore_file.is_file() {
        if snapshot::read(app)
            .and_then(|saved| restore_changes(app, &saved))
            .is_ok_and(|changes| changes.is_empty())
        {
            complete_without_restore(app)?;
            return Ok(());
        }
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
    snapshot::lock(app, &app.restore_lock, || {
        snapshot::recover_replacements(app)
    })?;
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
    process::tmux(app, &["wait-for", "-L", "atelier-restore-start"])?;
    let handled = process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore_handled"])
        .unwrap_or_default();
    let started = process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_restore_started"])
        .unwrap_or_default();
    if handled != "0" || started == "1" {
        process::tmux(app, &["wait-for", "-U", "atelier-restore-start"])?;
        return Ok(());
    }
    let changes = snapshot::read(app).and_then(|saved| restore_changes(app, &saved));
    if changes.as_ref().is_ok_and(|changes| changes.is_empty()) {
        let result = (|| {
            app.debug("restore skipped; live workspace topology matches snapshot")?;
            complete_without_restore(app)
        })();
        let unlock = process::tmux(app, &["wait-for", "-U", "atelier-restore-start"]);
        return result.and(unlock);
    }
    let needs_confirmation = mode == "prompt"
        || (mode == "always" && changes.as_ref().is_ok_and(RestoreChanges::destructive));
    if needs_confirmation && client.unwrap_or_default().is_empty() {
        process::tmux(app, &["wait-for", "-U", "atelier-restore-start"])?;
        return Ok(());
    }
    if app.set_global("@atelier_restore_started", "1").is_err() {
        let _ = process::tmux(app, &["wait-for", "-U", "atelier-restore-start"]);
        return Err(err("could not start restore"));
    }
    process::tmux(app, &["wait-for", "-U", "atelier-restore-start"])?;
    match mode.as_str() {
        "always" if !needs_confirmation => run(app, client, &HashMap::new(), &HashMap::new()),
        "never" => discard(app, client),
        "always" | "prompt" => {
            let client = client.unwrap();
            if display_restore_popup(app, client).is_err() {
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

struct RestoreChanges {
    missing: Vec<String>,
    mismatched: HashMap<String, snapshot::WorkspaceTopology>,
    processes: Vec<snapshot::ProcessChange>,
}

impl RestoreChanges {
    fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.mismatched.is_empty() && self.processes.is_empty()
    }

    fn destructive(&self) -> bool {
        !self.mismatched.is_empty()
            || self
                .processes
                .iter()
                .any(snapshot::ProcessChange::destructive)
    }
}

fn restore_changes(app: &App, saved: &snapshot::Snapshot) -> Result<RestoreChanges> {
    let mut changes = RestoreChanges {
        missing: Vec::new(),
        mismatched: HashMap::new(),
        processes: Vec::new(),
    };
    for saved in &saved.workspaces {
        if !app.workspaces.join(&saved.name).is_file() {
            continue;
        }
        match snapshot::current_workspace(app, &saved.name)? {
            None => changes.missing.push(saved.name.clone()),
            Some(current) => {
                let definition = workspace::read(app, &saved.name)?;
                let fallback =
                    (definition.destination == "local").then_some(definition.path.as_str());
                if !snapshot::topology_matches_snapshot_at(&current, saved, fallback) {
                    changes.mismatched.insert(saved.name.clone(), current);
                }
            }
        }
    }
    changes.processes = snapshot::process_changes(app, saved)?;
    Ok(changes)
}

fn complete_without_restore(app: &App) -> Result<()> {
    app.set_global("@atelier_restore_pending", "0")?;
    app.set_global("@atelier_restore_handled", "1")?;
    app.set_global("@atelier_restore_started", "1")?;
    app.snapshot("", "")
}

fn confirm_line(prompt: &str) -> Result<bool> {
    print!("{prompt} [y/N] ");
    io::stdout().flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y"))
}

fn display_restore_popup(app: &App, client: &str) -> Result<()> {
    name_bootstrap(app, client)?;
    let command = format!(
        "{} internal popup-restore {}",
        quote_sh(&app.cli_path()?),
        quote_sh(client)
    );
    process::tmux(
        app,
        &[
            "display-popup",
            "-c",
            client,
            "-E",
            "-w",
            "55%",
            "-h",
            "40%",
            &command,
        ],
    )
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
