use std::env;

use super::{lifecycle, tabs, App};
use crate::app::Direction;
use crate::config::quote_sh;
use crate::{process, workspace, Result};

pub(super) fn refresh_status(app: &App) -> Result<()> {
    let status_format = process::tmux_quiet(app, &["show-options", "-gqv", "@atelier_tabs_format"])
        .unwrap_or_default();
    if let Some(options) = process::tmux_quiet(app, &["show-options", "-g"]) {
        for line in options.lines() {
            if let Some(option) = line
                .split_whitespace()
                .next()
                .filter(|option| option.starts_with("@atelier_range_"))
            {
                let _ = process::tmux(app, &["set-option", "-gu", option]);
            }
        }
    }
    for session in workspace::session_names(app) {
        let line = status_line_for(app, &session)?;
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
    Ok(())
}

fn status_line_for(app: &App, active: &str) -> Result<String> {
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
    for (index, name) in workspace::all_names(app)?.into_iter().enumerate() {
        let token = format!("a{index}");
        process::tmux(
            app,
            &[
                "set-option",
                "-gq",
                &format!("@atelier_range_{token}"),
                &name,
            ],
        )?;
        let label = workspace_label(app, &name).replace('#', "##");
        let style = if name == active {
            &active_style
        } else if workspace::session_exists(app, &name) {
            &live_style
        } else {
            &stopped_style
        };
        let focus = if name == active { "#[list=focus]" } else { "" };
        let unfocus = if name == active { "#[list=on]" } else { "" };
        line.push_str(&format!(
            "{focus}#[range=user|{token},{style}] {label} #[default,norange]{separator}{unfocus}"
        ));
    }
    line.push_str(&format!(
        "#[range=user|new,{add_style}] + #[default,norange]#[nolist]"
    ));
    Ok(line)
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
    session: Option<&str>,
) -> Result<()> {
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
    let choices = ["Rename tab", "Close tab"];
    let Some(index) = choose_action(app, &name, &choices)? else {
        return Ok(());
    };
    match choices[index] {
        "Rename tab" => defer_request(app, "request-tab-rename", window, &client),
        "Close tab" => defer_request(app, "request-tab-close", window, &client),
        _ => Ok(()),
    }
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
    use super::adjacent_name;
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
}
