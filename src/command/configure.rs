use std::path::Path;

use super::{restore, ui, App};
use crate::config::quote_sh;
use crate::{process, Result};

pub(super) fn run(app: &App, root: &Path, cli: &Path) -> Result<()> {
    for (option, value) in [
        ("@atelier_tab_style", "default"),
        ("@atelier_tab_active_style", "reverse"),
        ("@atelier_workspace_active_style", "reverse"),
        ("@atelier_workspace_live_style", "bold"),
        ("@atelier_workspace_stopped_style", "dim"),
        ("@atelier_add_style", "bold"),
        ("@atelier_separator", "│"),
        ("@atelier_tab_separator", "│"),
        ("@atelier_restore", "prompt"),
        ("@atelier_status_sides", "off"),
        ("@atelier_new_workspace_key", "N"),
        ("@atelier_terminal_title", "#W - #S@#{@atelier_destination}"),
    ] {
        set_default(app, option, value)?;
    }

    let option =
        |name: &str| process::tmux_quiet(app, &["show-options", "-gqv", name]).unwrap_or_default();
    let tab_style = option("@atelier_tab_style");
    let active_style = option("@atelier_tab_active_style");
    let add_style = option("@atelier_add_style");
    let separator = option("@atelier_tab_separator").replace('#', "##");
    let title = option("@atelier_terminal_title");
    let tabs = format!(
        "{separator}#{{W:#[range=window|#{{window_index}} {tab_style}] #I #W #[norange default]{separator},#[list=focus range=window|#{{window_index}} {active_style}] #I #W #[norange default]{separator}#[list=on]}}#[range=user|new-tab {add_style}] + #[default,norange]"
    );
    let status = if option("@atelier_status_sides") == "on" {
        format!("#[align=left range=left #{{E:status-left-style}}]#[push-default]#{{T;=/#{{status-left-length}}:status-left}}#[pop-default]#[norange default]#[list=on align=#{{status-justify}}]{tabs}#[nolist align=right range=right #{{E:status-right-style}}]#[push-default]#{{T;=/#{{status-right-length}}:status-right}}#[pop-default]#[norange default]")
    } else {
        format!("#[list=on align=left]{tabs}#[nolist]")
    };
    let executable = quote_sh(&cli.to_string_lossy());
    let internal = |command: &str| format!("{executable} internal {command}");

    set(app, "@atelier_root", &root.to_string_lossy())?;
    set(app, "@atelier_cli", &cli.to_string_lossy())?;
    set(app, "@atelier_tabs_format", &status)?;
    for (option, value) in [
        ("mouse", "on"),
        ("status", "2"),
        ("status-interval", "5"),
        ("set-titles", "on"),
        ("set-titles-string", title.as_str()),
        ("status-format[0]", status.as_str()),
        (
            "status-format[1]",
            &format!("#[range=user|new,{add_style}] + #[default,norange]"),
        ),
    ] {
        process::tmux(app, &["set-option", "-g", option, value])?;
    }

    process::tmux(app, &["unbind-key", "-n", "MouseDown1Status"])?;
    let click = format!(
        "run-shell -b \"exec {} \\\"#{{mouse_status_range}}\\\" \\\"#{{client_name}}\\\" \\\"#{{session_name}}\\\"\"",
        internal("status-click")
    );
    process::tmux(
        app,
        &[
            "bind-key",
            "-n",
            "MouseUp1Status",
            "if-shell",
            "-F",
            "#{==:#{mouse_status_range},window}",
            "select-window -t =",
            &click,
        ],
    )?;
    let menu = format!(
        "run-shell -b \"exec {} \\\"#{{mouse_status_range}}\\\" \\\"#{{client_name}}\\\" \\\"#{{window_id}}\\\"\"",
        internal("status-menu")
    );
    process::tmux(app, &["bind-key", "-n", "MouseDown3Status", &menu])?;

    replace_native_bindings(app, &cli.to_string_lossy(), &executable)?;
    configure_new_workspace_binding(app, &cli.to_string_lossy(), &executable)?;
    restore::arm(app)?;

    let refresh = format!("run-shell -b \"{}\"", internal("refresh-status"));
    let save = format!("run-shell -b \"{}\"", internal("snapshot"));
    for hook in [
        "session-created",
        "session-closed",
        "session-renamed",
        "client-session-changed",
    ] {
        process::tmux(app, &["set-hook", "-g", &format!("{hook}[90]"), &refresh])?;
    }
    for hook in [
        "after-new-window",
        "after-split-window",
        "after-kill-pane",
        "after-select-layout",
        "window-layout-changed",
        "window-renamed",
        "session-window-changed",
        "window-pane-changed",
        "client-session-changed",
        "client-detached",
    ] {
        process::tmux(app, &["set-hook", "-g", &format!("{hook}[91]"), &save])?;
    }
    process::tmux(
        app,
        &[
            "set-hook",
            "-g",
            "client-attached[90]",
            &format!(
                "run-shell -b \"{} \\\"#{{client_name}}\\\"\"",
                internal("restore-start")
            ),
        ],
    )?;
    process::tmux(
        app,
        &[
            "set-hook",
            "-g",
            "session-created[91]",
            &format!(
                "run-shell -b -d 0.1 \"{} \\\"#{{session_name}}\\\"\"",
                internal("adopt-session")
            ),
        ],
    )?;
    process::tmux(
        app,
        &[
            "set-hook",
            "-g",
            "client-attached[91]",
            &format!(
                "run-shell -b \"{} \\\"#{{session_name}}\\\" \\\"#{{client_name}}\\\"\"",
                internal("adopt-session")
            ),
        ],
    )?;
    process::tmux(
        app,
        &[
            "run-shell",
            "-b",
            "-d",
            "0.5",
            &internal("restore-attached"),
        ],
    )?;
    ui::refresh_status(app)
}

fn set_default(app: &App, option: &str, value: &str) -> Result<()> {
    if process::tmux_quiet(app, &["show-options", "-gqv", option]).is_none_or(|v| v.is_empty()) {
        set(app, option, value)?;
    }
    Ok(())
}

fn set(app: &App, option: &str, value: &str) -> Result<()> {
    process::tmux(app, &["set-option", "-gq", option, value])
}

fn configure_new_workspace_binding(app: &App, cli: &str, executable: &str) -> Result<()> {
    let option =
        |name: &str| process::tmux_quiet(app, &["show-options", "-gqv", name]).unwrap_or_default();
    let key = option("@atelier_new_workspace_key");
    let previous = option("@atelier_new_workspace_bound_key");
    if !previous.is_empty() && owned_popup_binding(app, &previous, cli)? {
        process::tmux(app, &["unbind-key", &previous])?;
    }
    if key == "off" {
        return set(app, "@atelier_new_workspace_bound_key", "");
    }
    process::tmux(
        app,
        &[
            "bind-key",
            &key,
            "display-popup",
            "-E",
            "-w",
            "70%",
            "-h",
            "60%",
            &format!("exec {executable} internal popup-new"),
        ],
    )?;
    set(app, "@atelier_new_workspace_bound_key", &key)
}

fn owned_popup_binding(app: &App, key: &str, cli: &str) -> Result<bool> {
    let bindings = process::tmux_output(app, &["list-keys", "-T", "prefix"])?;
    Ok(bindings.lines().any(|binding| {
        let fields = binding.split_whitespace().collect::<Vec<_>>();
        let Some(table) = fields.iter().position(|field| *field == "-T") else {
            return false;
        };
        fields
            .get(table + 2)
            .map(|value| value.trim_start_matches('\\'))
            == Some(key)
            && binding.contains(cli)
            && binding.contains("internal popup-new")
    }))
}

fn replace_native_bindings(app: &App, cli: &str, executable: &str) -> Result<()> {
    let bindings = process::tmux_output(app, &["list-keys", "-T", "prefix"])?;
    for binding in bindings.lines() {
        let fields = binding.split_whitespace().collect::<Vec<_>>();
        let Some(table) = fields.iter().position(|field| *field == "-T") else {
            continue;
        };
        if fields.get(table + 1) != Some(&"prefix") {
            continue;
        }
        let Some(key) = fields
            .get(table + 2)
            .map(|key| key.trim_start_matches('\\'))
        else {
            continue;
        };
        let action = fields[table + 3..].join(" ");
        let owned = action.contains(cli);
        let command = if action == "new-window" || owned && action.contains(" window ") {
            Some("window \"#{session_name}\"")
        } else if action.starts_with("split-window -h")
            || owned && action.contains(" split vertical ")
        {
            Some("split vertical \"#{pane_id}\"")
        } else if action.starts_with("split-window")
            || owned && action.contains(" split horizontal ")
        {
            Some("split horizontal \"#{pane_id}\"")
        } else if action.starts_with("command-prompt") && action.contains(" rename-window ")
            || owned && action.contains("request-tab-rename")
        {
            Some("internal request-tab-rename \"#{window_id}\" \"#{client_name}\"")
        } else if action.starts_with("confirm-before") && action.contains("kill-window")
            || owned && action.contains("request-tab-close")
        {
            Some("internal request-tab-close \"#{window_id}\" \"#{client_name}\"")
        } else if action.starts_with("command-prompt") && action.contains(" rename-session ")
            || owned && action.contains("request-rename")
        {
            Some("internal request-rename \"#{session_name}\" \"#{client_name}\"")
        } else if action == "next-window" || owned && action.contains("navigate-tab next") {
            Some("internal navigate-tab next \"#{session_name}\"")
        } else if action == "previous-window" || owned && action.contains("navigate-tab previous") {
            Some("internal navigate-tab previous \"#{session_name}\"")
        } else if action == "switch-client -n"
            || owned && action.contains("navigate-workspace next")
        {
            Some("internal navigate-workspace next \"#{session_name}\" \"#{client_name}\"")
        } else if action == "switch-client -p"
            || owned && action.contains("navigate-workspace previous")
        {
            Some("internal navigate-workspace previous \"#{session_name}\" \"#{client_name}\"")
        } else {
            None
        };
        if let Some(command) = command {
            process::tmux(
                app,
                &[
                    "bind-key",
                    key,
                    "run-shell",
                    "-b",
                    &format!("exec {executable} {command}"),
                ],
            )?;
        }
    }
    Ok(())
}
