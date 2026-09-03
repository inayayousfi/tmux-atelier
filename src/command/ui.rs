use std::env;

use super::{App, configure, lifecycle, restart, tabs};
use crate::app::Direction;
use crate::config::quote_sh;
use crate::process_state::RestartPolicy;
use crate::{Result, process, workspace};

pub(super) fn refresh_status(app: &App) -> Result<()> {
    crate::snapshot::lock(app, &app.status_lock, || refresh_status_locked(app))
}

fn refresh_status_locked(app: &App) -> Result<()> {
    let clients = client_ids(app);
    let status_format = configure::tabs_format(app, &clients);
    process::tmux(
        app,
        &["set-option", "-gq", "@atelier_tabs_format", &status_format],
    )?;
    let generation =
        process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_range_generation"])
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0)
            .wrapping_add(1);
    process::tmux(
        app,
        &[
            "set-option",
            "-gq",
            "@atelier_range_generation",
            &generation.to_string(),
        ],
    )?;
    let names = workspace::all_names(app)?;
    let tokens = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let token = format!("a{generation:x}_{index:x}");
            process::tmux(
                app,
                &[
                    "set-option",
                    "-gq",
                    &format!("@atelier_range_{token}"),
                    name,
                ],
            )?;
            Ok(token)
        })
        .collect::<Result<Vec<_>>>()?;
    for session in workspace::session_names(app) {
        let line = status_line_for(app, &session, &names, &tokens, &clients)?;
        let target = format!("={session}:");
        process::tmux(
            app,
            &[
                "set-option",
                "-q",
                "-t",
                &target,
                "status-format[0]",
                &status_format,
            ],
        )?;
        process::tmux(
            app,
            &["set-option", "-q", "-t", &target, "status-format[1]", &line],
        )?;
    }
    if let Some(options) = process::tmux_quiet(app, &["show-options", "-g"]) {
        let current = format!("@atelier_range_a{generation:x}_");
        let previous = format!("@atelier_range_a{:x}_", generation.wrapping_sub(1));
        let dragging = options
            .lines()
            .any(|line| line.starts_with("@atelier_drag_kind_"));
        for line in options.lines() {
            if let Some(option) = line.split_whitespace().next().filter(|option| {
                option.starts_with("@atelier_range_")
                    && *option != "@atelier_range_generation"
                    && !dragging
                    && !option.starts_with(&current)
                    && !option.starts_with(&previous)
            }) {
                let _ = process::tmux(app, &["set-option", "-gu", option]);
            }
        }
    }
    Ok(())
}

fn status_line_for(
    app: &App,
    active: &str,
    names: &[String],
    tokens: &[String],
    clients: &[String],
) -> Result<String> {
    let option = |name: &str, fallback: &str| {
        process::tmux_quiet(app, &["show-options", "-gqv", name])
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fallback.into())
    };
    let active_style = option("@atelier_workspace_active_style", "reverse");
    let live_style = option("@atelier_workspace_live_style", "bold");
    let stopped_style = option("@atelier_workspace_stopped_style", "dim");
    let add_style = option("@atelier_add_style", "bold");
    let separator = option("@atelier_separator", "│").replace('#', "##");
    let mut line = format!("#[list=on align=left]{separator}");
    for (name, token) in names.iter().zip(tokens) {
        let label = workspace_label(app, name).replace('#', "##");
        let style = if name == active {
            &active_style
        } else if workspace::session_exists(app, name) {
            &live_style
        } else {
            &stopped_style
        };
        let focus = if name == active { "#[list=focus]" } else { "" };
        let unfocus = if name == active { "#[list=on]" } else { "" };
        let overlay = drag_overlays(clients, name, token, "@atelier_workspace_active_style");
        line.push_str(&format!(
            "{focus}#[range=user|{token},{style}]{overlay} {label} #[default,norange]{separator}{unfocus}"
        ));
    }
    line.push_str(&format!(
        "#[range=user|new,{add_style}] + #[default,norange]#[nolist]"
    ));
    Ok(line)
}

pub(super) fn client_ids(app: &App) -> Vec<String> {
    let mut clients = process::tmux_quiet(app, &["list-clients", "-F", "#{client_pid}"])
        .unwrap_or_default()
        .lines()
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    clients.sort();
    clients.dedup();
    clients
}

pub(super) fn drag_overlays(
    clients: &[String],
    source_identity: &str,
    target_identity: &str,
    active_style: &str,
) -> String {
    let mut format = String::new();
    for client in clients {
        format.push_str(&format!(
            "#{{?#{{&&:#{{==:#{{client_pid}},{client}}},#{{==:#{{@atelier_drag_target_{client}}},{target_identity}}}}},#[#{{E:{active_style}}}#,#{{E:@atelier_drag_target_style}}],}}"
        ));
        format.push_str(&format!(
            "#{{?#{{&&:#{{==:#{{client_pid}},{client}}},#{{==:#{{@atelier_drag_source_{client}}},{source_identity}}}}},#[#{{E:@atelier_drag_source_style}}],}}"
        ));
    }
    format
}

pub(super) fn navigate_workspace(
    app: &App,
    direction: Direction,
    active: &str,
    client: Option<&str>,
) -> Result<()> {
    let names = workspace::all_names(app)?;
    let Some(target) = adjacent_name(&names, active, direction) else {
        return Ok(());
    };
    if target == active {
        return Ok(());
    }
    if !workspace::session_exists(app, target) {
        lifecycle::open(app, target, Some("--detached"))?;
    }
    let mut owned = vec!["switch-client".to_owned()];
    if let Some(client) = client.filter(|value| !value.is_empty()) {
        owned.extend(["-c".into(), client.into()]);
    }
    owned.extend(["-t".into(), format!("={target}")]);
    process::tmux(app, &owned.iter().map(String::as_str).collect::<Vec<_>>())
}

fn adjacent_name<'a>(names: &'a [String], active: &str, direction: Direction) -> Option<&'a str> {
    let fallback = match direction {
        Direction::Next => names.first(),
        Direction::Previous => names.last(),
    }?;
    let Some(index) = names.iter().position(|name| name == active) else {
        return Some(fallback);
    };
    Some(match direction {
        Direction::Next => &names[(index + 1) % names.len()],
        Direction::Previous => &names[(index + names.len() - 1) % names.len()],
    })
}

fn workspace_label(app: &App, name: &str) -> String {
    if app.workspaces.join(name).is_file() {
        match workspace::read(app, name) {
            Ok(definition) => {
                if workspace::session_exists(app, name)
                    && (workspace::session_option(app, name, "@atelier_managed") != "1"
                        || workspace::session_option(app, name, "@atelier_destination")
                            != definition.destination
                        || workspace::session_option(app, name, "@atelier_path") != definition.path)
                {
                    return format!("{name}@conflict");
                }
                format!(
                    "{name}@{}",
                    definition
                        .destination
                        .rsplit('@')
                        .next()
                        .unwrap_or(&definition.destination)
                )
            }
            Err(_) => format!("{name}@invalid"),
        }
    } else if workspace::session_option(app, name, "@atelier_managed") == "1" {
        let destination = workspace::session_option(app, name, "@atelier_destination");
        format!(
            "{name}@{}",
            destination.rsplit('@').next().unwrap_or(&destination)
        )
    } else {
        format!("{name}@tmux")
    }
}

pub(super) fn status_click(
    app: &App,
    token: &str,
    client: Option<&str>,
    client_id: &str,
    session: Option<&str>,
    window: Option<&str>,
) -> Result<()> {
    clear_drag(app, client_id, client)?;
    if token == "window" {
        let window = window.unwrap_or_default();
        tabs::validate_window(window)?;
        if let Some(client) = client.filter(|value| !value.is_empty()) {
            return process::tmux(app, &["switch-client", "-c", client, "-t", window]);
        }
        return process::tmux(app, &["select-window", "-t", window]);
    }
    if token == "new" {
        return show_new_popup(app, client);
    }
    if token == "new-tab" {
        let owned;
        let selected = if session.unwrap_or_default().is_empty()
            && client.is_some_and(|value| !value.is_empty())
        {
            owned = process::tmux_output(
                app,
                &[
                    "display-message",
                    "-p",
                    "-c",
                    client.unwrap(),
                    "#{session_name}",
                ],
            )?;
            Some(owned.as_str())
        } else {
            session
        };
        return tabs::window(app, selected);
    }
    let name = range_name(app, token);
    if name.is_empty() {
        return Ok(());
    }
    if workspace::session_exists(app, &name) {
        let mut owned = vec!["switch-client".to_owned()];
        if let Some(client) = client.filter(|value| !value.is_empty()) {
            owned.extend(["-c".into(), client.into()]);
        }
        owned.extend(["-t".into(), format!("={name}")]);
        process::tmux(app, &owned.iter().map(String::as_str).collect::<Vec<_>>())
    } else {
        lifecycle::open(app, &name, None)
    }
}

pub(super) fn drag_start(
    app: &App,
    token: &str,
    client: &str,
    client_id: &str,
    window: Option<&str>,
) -> Result<()> {
    validate_client_id(client_id)?;
    clear_drag(app, client_id, None)?;
    let (kind, source) = if token == "window" {
        let window = window.unwrap_or_default();
        tabs::validate_window(window)?;
        ("tab", window.to_owned())
    } else {
        let name = range_name(app, token);
        if name.is_empty() {
            return Ok(());
        }
        workspace::validate_name(&name)?;
        ("workspace", name)
    };
    process::tmux(
        app,
        &["set-option", "-gq", &drag_option("kind", client_id), kind],
    )?;
    process::tmux(
        app,
        &[
            "set-option",
            "-gq",
            &drag_option("source", client_id),
            &source,
        ],
    )?;
    configure_drag_table(app, client_id)?;
    refresh_status(app)?;
    refresh_client(app, client)?;
    if client.is_empty() {
        Ok(())
    } else {
        process::tmux(
            app,
            &["switch-client", "-c", client, "-T", &drag_table(client_id)],
        )
    }
}

pub(super) fn drag_end(
    app: &App,
    token: &str,
    client: &str,
    client_id: &str,
    window: Option<&str>,
) -> Result<()> {
    validate_client_id(client_id)?;
    let kind = drag_value(app, "kind", client_id);
    let source = drag_value(app, "source", client_id);
    clear_drag(app, client_id, Some(client))?;
    if kind.is_empty() || source.is_empty() {
        return Ok(());
    }
    if kind == "tab" && token == "window" {
        return tabs::reorder(app, &source, window.unwrap_or_default());
    }
    if kind == "workspace" && token != "window" {
        let target = range_name(app, token);
        if !target.is_empty() {
            workspace::reorder(app, &source, &target)?;
            return refresh_status(app);
        }
    }
    Ok(())
}

pub(super) fn drag_update(
    app: &App,
    token: &str,
    client: &str,
    client_id: &str,
    window: Option<&str>,
) -> Result<()> {
    validate_client_id(client_id)?;
    let kind = drag_value(app, "kind", client_id);
    if drag_value(app, "source", client_id).is_empty() {
        return Ok(());
    }
    let target = if kind == "tab" && token == "window" {
        let window = window.unwrap_or_default();
        if window.is_empty() {
            None
        } else {
            tabs::validate_window(window)?;
            Some(window.to_owned())
        }
    } else if kind == "workspace" && token != "window" {
        let name = range_name(app, token);
        (!name.is_empty()).then_some(token.to_owned())
    } else {
        None
    };
    let current = drag_value(app, "target", client_id);
    if target.as_deref().unwrap_or_default() != current {
        if let Some(target) = target {
            process::tmux(
                app,
                &[
                    "set-option",
                    "-gq",
                    &drag_option("target", client_id),
                    &target,
                ],
            )?;
        } else {
            process::tmux(
                app,
                &["set-option", "-gu", &drag_option("target", client_id)],
            )?;
        }
        refresh_client(app, client)?;
    }
    if client.is_empty() {
        Ok(())
    } else {
        process::tmux(
            app,
            &["switch-client", "-c", client, "-T", &drag_table(client_id)],
        )
    }
}

pub(super) fn drag_cancel(app: &App, client_id: &str, client: Option<&str>) -> Result<()> {
    validate_client_id(client_id)?;
    clear_drag(app, client_id, client)?;
    if client.is_none_or(str::is_empty) {
        refresh_status(app)?;
    }
    Ok(())
}

pub(super) fn cleanup_drags(app: &App) -> Result<()> {
    let active: std::collections::HashSet<_> = client_ids(app).into_iter().collect();
    if let Some(options) = process::tmux_quiet(app, &["show-options", "-g"]) {
        let stale = options
            .lines()
            .filter_map(|line| {
                line.split_whitespace()
                    .next()?
                    .strip_prefix("@atelier_drag_kind_")
            })
            .filter(|client_id| !active.contains(*client_id))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for client_id in stale {
            clear_drag(app, &client_id, None)?;
        }
    }
    refresh_status(app)
}

fn configure_drag_table(app: &App, client_id: &str) -> Result<()> {
    let table = drag_table(client_id);
    process::tmux_success(app, &["unbind-key", "-a", "-T", &table]);
    let target = drag_option("target", client_id);
    let clear_motion =
        format!("set-option -gu {target} ; refresh-client -S ; switch-client -T {table}");
    let executable = quote_sh(&app.cli_path()?);
    let update = format!(
        "run-shell \"exec {executable} internal drag-update \\\"#{{mouse_status_range}}\\\" \\\"#{{client_name}}\\\" {client_id} \\\"#{{window_id}}\\\"\""
    );
    process::tmux(
        app,
        &["bind-key", "-r", "-T", &table, "MouseDrag1Status", &update],
    )?;
    for key in [
        "MouseDrag1StatusDefault",
        "MouseDrag1StatusLeft",
        "MouseDrag1StatusRight",
        "MouseDrag1Pane",
        "MouseDrag1Border",
        "MouseDrag1ScrollbarSlider",
        "MouseDrag1ScrollbarUp",
        "MouseDrag1ScrollbarDown",
    ] {
        bind_drag_action(app, &table, key, &clear_motion)?;
    }
    for control in 0..=9 {
        bind_drag_action(
            app,
            &table,
            &format!("MouseDrag1Control{control}"),
            &clear_motion,
        )?;
    }
    let drop = format!(
        "run-shell \"exec {executable} internal drag-end \\\"#{{mouse_status_range}}\\\" \\\"#{{client_name}}\\\" {client_id} \\\"#{{window_id}}\\\"\""
    );
    process::tmux(
        app,
        &["bind-key", "-T", &table, "MouseDragEnd1Status", &drop],
    )?;
    let cancel = format!(
        "run-shell \"exec {executable} internal drag-cancel {client_id} \\\"#{{client_name}}\\\"\""
    );
    for key in [
        "MouseDragEnd1StatusDefault",
        "MouseDragEnd1StatusLeft",
        "MouseDragEnd1StatusRight",
        "MouseDragEnd1Pane",
        "MouseDragEnd1Border",
        "MouseDragEnd1ScrollbarSlider",
        "MouseDragEnd1ScrollbarUp",
        "MouseDragEnd1ScrollbarDown",
        "MouseUp1StatusDefault",
        "MouseUp1StatusLeft",
        "MouseUp1StatusRight",
        "MouseUp1Pane",
        "MouseUp1Border",
        "MouseUp1ScrollbarSlider",
        "MouseUp1ScrollbarUp",
        "MouseUp1ScrollbarDown",
    ] {
        process::tmux(app, &["bind-key", "-T", &table, key, &cancel])?;
    }
    for control in 0..=9 {
        for event in ["MouseDragEnd1Control", "MouseUp1Control"] {
            process::tmux(
                app,
                &[
                    "bind-key",
                    "-T",
                    &table,
                    &format!("{event}{control}"),
                    &cancel,
                ],
            )?;
        }
    }
    let click = format!(
        "run-shell \"exec {executable} internal status-click \\\"#{{mouse_status_range}}\\\" \\\"#{{client_name}}\\\" {client_id} \\\"#{{session_name}}\\\" \\\"#{{window_id}}\\\"\""
    );
    process::tmux(app, &["bind-key", "-T", &table, "MouseUp1Status", &click])
}

fn bind_drag_action(app: &App, table: &str, key: &str, action: &str) -> Result<()> {
    process::tmux(
        app,
        &[
            "bind-key", "-r", "-T", table, key, "if-shell", "-F", "1", action,
        ],
    )
}

fn clear_drag(app: &App, client_id: &str, client: Option<&str>) -> Result<()> {
    validate_client_id(client_id)?;
    for field in ["kind", "source", "target"] {
        process::tmux(app, &["set-option", "-gu", &drag_option(field, client_id)])?;
    }
    process::tmux_success(app, &["unbind-key", "-a", "-T", &drag_table(client_id)]);
    if let Some(client) = client.filter(|value| !value.is_empty()) {
        let _ = process::tmux(app, &["refresh-client", "-S", "-t", client]);
    }
    Ok(())
}

fn refresh_client(app: &App, client: &str) -> Result<()> {
    if client.is_empty() {
        Ok(())
    } else {
        process::tmux(app, &["refresh-client", "-S", "-t", client])
    }
}

fn drag_value(app: &App, field: &str, client_id: &str) -> String {
    process::tmux_quiet(
        app,
        &["show-options", "-gqv", &drag_option(field, client_id)],
    )
    .unwrap_or_default()
}

fn drag_option(field: &str, client_id: &str) -> String {
    format!("@atelier_drag_{field}_{client_id}")
}

fn drag_table(client_id: &str) -> String {
    format!("atelier-drag-{client_id}")
}

fn validate_client_id(client_id: &str) -> Result<()> {
    if !client_id.is_empty() && client_id.bytes().all(|byte| byte.is_ascii_digit()) {
        Ok(())
    } else {
        Err(crate::err(format!("invalid tmux client id: {client_id}")))
    }
}

fn range_name(app: &App, token: &str) -> String {
    process::tmux_quiet(
        app,
        &["show-options", "-gqv", &format!("@atelier_range_{token}")],
    )
    .unwrap_or_default()
}

fn show_new_popup(app: &App, client: Option<&str>) -> Result<()> {
    let command = format!("{} internal popup-new", quote_sh(&app.cli_path()?));
    let mut owned = vec!["display-popup".to_owned()];
    if let Some(client) = client.filter(|value| !value.is_empty()) {
        owned.extend([
            "-c".into(),
            client.into(),
            "-e".into(),
            format!("TMUX_ATELIER_CLIENT={client}"),
        ]);
    }
    owned.extend([
        "-E".into(),
        "-w".into(),
        "70%".into(),
        "-h".into(),
        "60%".into(),
        command,
    ]);
    process::tmux(app, &owned.iter().map(String::as_str).collect::<Vec<_>>())
}

pub(super) fn status_menu(
    app: &App,
    token: &str,
    client: Option<&str>,
    window: Option<&str>,
) -> Result<()> {
    if token == "window" {
        return tab_menu(app, window.unwrap_or_default(), client);
    }
    let name = range_name(app, token);
    app.debug(&format!(
        "status-menu token={token} resolved={name} requested_client={}",
        client.unwrap_or_default()
    ))?;
    if name.is_empty() {
        Ok(())
    } else {
        menu(app, &name, client)
    }
}

fn tab_menu(app: &App, window: &str, client: Option<&str>) -> Result<()> {
    tabs::validate_window(window)?;
    let command = format!(
        "{} internal popup-tab-menu {}",
        quote_sh(&app.cli_path()?),
        quote_sh(window)
    );
    let mut owned = vec!["display-popup".to_owned()];
    if let Some(client) = client.filter(|value| !value.is_empty()) {
        owned.extend([
            "-c".into(),
            client.into(),
            "-e".into(),
            format!("TMUX_ATELIER_CLIENT={client}"),
        ]);
    }
    app.debug(&format!(
        "tab-menu opening window={window} target_client={}",
        client.unwrap_or_default()
    ))?;
    owned.extend([
        "-E".into(),
        "-w".into(),
        "45%".into(),
        "-h".into(),
        "30%".into(),
        command,
    ]);
    process::tmux(app, &owned.iter().map(String::as_str).collect::<Vec<_>>())
}

pub(super) fn menu(app: &App, name: &str, client: Option<&str>) -> Result<()> {
    workspace::validate_name(name)?;
    let command = format!(
        "{} internal popup-workspace-menu {}",
        quote_sh(&app.cli_path()?),
        quote_sh(name)
    );
    let mut owned = vec!["display-popup".to_owned()];
    if let Some(client) = client.filter(|value| !value.is_empty()) {
        owned.extend([
            "-c".into(),
            client.into(),
            "-e".into(),
            format!("TMUX_ATELIER_CLIENT={client}"),
        ]);
    }
    app.debug(&format!(
        "workspace-menu opening name={name} target_client={}",
        client.unwrap_or_default()
    ))?;
    owned.extend([
        "-E".into(),
        "-w".into(),
        "45%".into(),
        "-h".into(),
        "35%".into(),
        command,
    ]);
    process::tmux(app, &owned.iter().map(String::as_str).collect::<Vec<_>>())?;
    app.debug(&format!(
        "workspace-menu closed name={name} target_client={} status=0",
        client.unwrap_or_default()
    ))
}

fn choose_action(app: &App, prompt: &str, choices: &[&str]) -> Result<Option<usize>> {
    app.choose(
        prompt,
        &choices
            .iter()
            .map(|choice| (*choice).into())
            .collect::<Vec<_>>(),
    )
}

pub(super) fn popup_workspace_menu(app: &App, name: &str) -> Result<()> {
    workspace::validate_name(name)?;
    let client = env::var("TMUX_ATELIER_CLIENT").unwrap_or_default();
    app.debug(&format!(
        "workspace-menu started name={name} inherited_client={client}"
    ))?;
    let mut choices = Vec::new();
    if !workspace::session_exists(app, name) {
        choices.push("Open workspace");
    }
    choices.push("Edit workspace");
    if workspace::session_exists(app, name) {
        choices.push("Stop workspace");
    }
    if app.workspaces.join(name).is_file() {
        choices.push("Delete workspace");
    }
    let Some(index) = choose_action(app, name, &choices)? else {
        app.debug(&format!(
            "workspace-menu cancelled name={name} inherited_client={client}"
        ))?;
        return Ok(());
    };
    let action = choices[index];
    app.debug(&format!(
        "workspace-menu selected name={name} action={action} inherited_client={client}"
    ))?;
    match action {
        "Open workspace" => lifecycle::open(app, name, None),
        "Edit workspace" => popup_workspace_edit_menu(app, name),
        "Stop workspace" => defer_request(app, "request-close", name, &client),
        "Delete workspace" => defer_request(app, "request-delete", name, &client),
        _ => Ok(()),
    }
}

fn popup_workspace_edit_menu(app: &App, name: &str) -> Result<()> {
    let client = env::var("TMUX_ATELIER_CLIENT").unwrap_or_default();
    let choices = if app.workspaces.join(name).is_file() {
        vec!["Change target", "Rename workspace"]
    } else {
        vec!["Rename workspace"]
    };
    match choose_action(app, &format!("Edit {name}"), &choices)? {
        Some(index) if choices[index] == "Change target" => lifecycle::popup_edit(app, name),
        Some(index) if choices[index] == "Rename workspace" => {
            defer_request(app, "request-rename", name, &client)
        }
        _ => Ok(()),
    }
}

pub(super) fn popup_tab_menu(app: &App, window: &str) -> Result<()> {
    tabs::validate_window(window)?;
    let name = process::tmux_output(
        app,
        &["display-message", "-p", "-t", window, "#{window_name}"],
    )?;
    let client = env::var("TMUX_ATELIER_CLIENT").unwrap_or_default();
    app.debug(&format!(
        "tab-menu started window={window} name={name} inherited_client={client}"
    ))?;
    let choices = ["Rename tab", "Restart after restore", "Close tab"];
    let Some(index) = choose_action(app, &name, &choices)? else {
        return Ok(());
    };
    match choices[index] {
        "Rename tab" => defer_request(app, "request-tab-rename", window, &client),
        "Restart after restore" => popup_restart_policy(app, window),
        "Close tab" => defer_request(app, "request-tab-close", window, &client),
        _ => Ok(()),
    }
}

fn popup_restart_policy(app: &App, window: &str) -> Result<()> {
    let pane = process::tmux_output(app, &["display-message", "-p", "-t", window, "#{pane_id}"])?;
    let choices = ["Auto", "Always", "Never"];
    let Some(index) = choose_action(app, "Restart active pane", &choices)? else {
        return Ok(());
    };
    let policy = match choices[index] {
        "Auto" => RestartPolicy::Auto,
        "Always" => RestartPolicy::Always,
        "Never" => RestartPolicy::Never,
        _ => unreachable!(),
    };
    restart::set(app, policy, Some(&pane))
}

fn defer_request(app: &App, request: &str, target: &str, client: &str) -> Result<()> {
    let mut command = format!(
        "{} internal {request} {}",
        quote_sh(&app.cli_path()?),
        quote_sh(target)
    );
    if !client.is_empty() {
        command.push(' ');
        command.push_str(&quote_sh(client));
    }
    command.push_str(&format!(
        " >>{} 2>&1",
        quote_sh(&app.debug_log.to_string_lossy())
    ));
    app.debug(&format!(
        "defer scheduling request={request} target={target} target_client={client}"
    ))?;
    process::tmux(app, &["run-shell", "-b", "-d", "0.1", &command])
}

pub(super) fn popup_request(
    app: &App,
    subcommand: &str,
    target: &str,
    client: Option<&str>,
    width: &str,
    height: &str,
) -> Result<()> {
    let command = format!(
        "{} internal {subcommand} {}",
        quote_sh(&app.cli_path()?),
        quote_sh(target)
    );
    let mut owned = vec!["display-popup".to_owned()];
    if let Some(client) = client.filter(|value| !value.is_empty()) {
        owned.extend(["-c".into(), client.into()]);
    }
    owned.extend([
        "-E".into(),
        "-w".into(),
        width.into(),
        "-h".into(),
        height.into(),
        command,
    ]);
    process::tmux(app, &owned.iter().map(String::as_str).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::{adjacent_name, drag_overlays};
    use crate::app::Direction;

    #[test]
    fn adjacent_workspace_wraps_and_handles_an_unknown_active_name() {
        let names = vec!["one".into(), "two".into(), "three".into()];
        assert_eq!(
            adjacent_name(&names, "one", Direction::Previous),
            Some("three")
        );
        assert_eq!(adjacent_name(&names, "three", Direction::Next), Some("one"));
        assert_eq!(
            adjacent_name(&names, "missing", Direction::Next),
            Some("one")
        );
        assert_eq!(
            adjacent_name(&names, "missing", Direction::Previous),
            Some("three")
        );
        assert_eq!(adjacent_name(&[], "missing", Direction::Next), None);
    }

    #[test]
    fn drag_overlays_are_client_specific_semantic_styles_with_source_precedence() {
        let format = drag_overlays(
            &["123".into(), "456".into()],
            "workspace",
            "a1_0",
            "@atelier_workspace_active_style",
        );
        assert!(format.contains("#{==:#{client_pid},123}"));
        assert!(format.contains("#{@atelier_drag_target_456}"));
        assert!(format.contains("#{E:@atelier_workspace_active_style}"));
        assert!(format.contains("#{E:@atelier_drag_target_style}"));
        assert!(format.contains("#{E:@atelier_drag_source_style}"));
        assert!(format.find("drag_target_123").unwrap() < format.find("drag_source_123").unwrap());
        assert!(!format.contains("colour"));
        assert!(!format.contains("rgb"));
        assert!(!format.contains('#') || !format.contains("#ff"));
    }
}
