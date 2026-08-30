mod lifecycle;
mod restore;
mod tabs;
mod ui;

use std::fs::{self, OpenOptions};
use std::ops::Deref;

use crate::config::Config;
use crate::{err, process, snapshot, workspace, Result};

pub(crate) struct App {
    config: Config,
}

impl Deref for App {
    type Target = Config;

    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

impl App {
    pub(crate) fn from_env() -> Result<Self> {
        Ok(Self {
            config: Config::from_env()?,
        })
    }

    pub(crate) fn dispatch(&self, command: &str, args: &[String]) -> Result<()> {
        match command {
            "new" if (1..=3).contains(&args.len()) => lifecycle::new(self, args, None),
            "open" if (1..=2).contains(&args.len()) => {
                lifecycle::open(self, &args[0], args.get(1).map(String::as_str))
            }
            "window" if args.len() <= 1 => tabs::window(self, args.first().map(String::as_str)),
            "split" if (1..=2).contains(&args.len()) => {
                tabs::split(self, &args[0], args.get(1).map(String::as_str))
            }
            "rename" if args.len() == 2 => lifecycle::rename(self, &args[0], &args[1]),
            "edit" if args.len() == 2 => lifecycle::edit(self, &args[0], &args[1], None),
            "close" if args.len() == 1 => lifecycle::close(self, &args[0]),
            "delete" if args.len() == 1 => lifecycle::delete(self, &args[0]),
            "popup-new" if args.is_empty() => lifecycle::popup_new(self),
            "popup-workspace-menu" if args.len() == 1 => ui::popup_workspace_menu(self, &args[0]),
            "popup-tab-menu" if args.len() == 1 => ui::popup_tab_menu(self, &args[0]),
            "popup-restore" if args.len() <= 1 => {
                restore::popup_restore(self, args.first().map(String::as_str))
            }
            "popup-tab-rename" if args.len() == 1 => tabs::popup_rename(self, &args[0]),
            "popup-rename" if args.len() == 1 => lifecycle::popup_rename(self, &args[0]),
            "popup-edit" if args.len() == 1 => lifecycle::popup_edit(self, &args[0]),
            "refresh-status" if args.is_empty() => ui::refresh_status(self),
            "status-click" if (1..=3).contains(&args.len()) => ui::status_click(
                self,
                &args[0],
                args.get(1).map(String::as_str),
                args.get(2).map(String::as_str),
            ),
            "status-menu" if (1..=2).contains(&args.len()) => {
                ui::status_menu(self, &args[0], args.get(1).map(String::as_str))
            }
            "menu" if (1..=2).contains(&args.len()) => {
                ui::menu(self, &args[0], args.get(1).map(String::as_str))
            }
            "request-close" if (1..=2).contains(&args.len()) => {
                lifecycle::request_close(self, &args[0], args.get(1).map(String::as_str))
            }
            "request-rename" if (1..=2).contains(&args.len()) => {
                lifecycle::request_rename(self, &args[0], args.get(1).map(String::as_str))
            }
            "request-tab-rename" if (1..=2).contains(&args.len()) => {
                tabs::request_rename(self, &args[0], args.get(1).map(String::as_str))
            }
            "request-tab-close" if (1..=2).contains(&args.len()) => {
                tabs::request_close(self, &args[0], args.get(1).map(String::as_str))
            }
            "tab-close" if args.len() == 1 => tabs::close(self, &args[0]),
            "request-delete" if (1..=2).contains(&args.len()) => {
                lifecycle::request_delete(self, &args[0], args.get(1).map(String::as_str))
            }
            "snapshot" if args.is_empty() => self.snapshot("", ""),
            "restore-arm" if args.is_empty() => restore::arm(self),
            "restore-start" if args.len() <= 1 => {
                restore::start(self, args.first().map(String::as_str))
            }
            "restore-attached" if args.is_empty() => restore::attached(self),
            "restore" if args.len() <= 1 => restore::run(self, args.first().map(String::as_str)),
            "restore-discard" if args.is_empty() => restore::discard(self, None),
            "adopt-session" if (1..=2).contains(&args.len()) => {
                restore::adopt(self, &args[0], args.get(1).map(String::as_str))
            }
            "debug-path" if args.is_empty() => {
                println!("{}", self.debug_log.display());
                Ok(())
            }
            "debug-clear" if args.is_empty() => self.debug_clear(),
            "help" | "-h" | "--help" if args.is_empty() => {
                Self::usage();
                Ok(())
            }
            known if is_known(known) => Err(err(format!("usage: tmux-atelier {known}"))),
            _ => Err(err(format!("unknown command: {command}"))),
        }
    }

    fn snapshot(&self, exclude_session: &str, exclude_window: &str) -> Result<()> {
        snapshot::lock(self, &self.snapshot_lock, || {
            snapshot::write(self, exclude_session, exclude_window)
        })
    }

    fn refresh_status_if_running(&self) -> Result<()> {
        if process::tmux_success(self, &["list-sessions"]) {
            ui::refresh_status(self)
        } else {
            Ok(())
        }
    }

    fn set_global(&self, option: &str, value: &str) -> Result<()> {
        process::tmux(self, &["set-option", "-gq", option, value])
    }

    fn cli_path(&self) -> Result<String> {
        process::tmux_quiet(self, &["show-options", "-gqv", "@atelier_cli"])
            .filter(|value| !value.is_empty())
            .ok_or_else(|| err("@atelier_cli is not configured"))
    }

    fn debug_clear(&self) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        self.secure_dir(&self.state_root)?;
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.debug_log)?;
        fs::set_permissions(&self.debug_log, fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    pub(crate) fn usage() {
        println!("Usage: tmux-atelier COMMAND [ARGUMENTS]\n\nCommands:\n  new TARGET [NAME] [--detached]\n  open NAME [--detached]\n  window [SESSION]\n  split vertical|horizontal [PANE]\n  rename OLD NEW\n  edit NAME TARGET\n  close NAME\n  delete NAME\n  popup-new\n  popup-workspace-menu NAME\n  popup-tab-menu WINDOW_ID\n  snapshot\n  restore-start [CLIENT]\n  adopt-session SESSION [CLIENT]\n  refresh-status");
    }
}

fn is_known(command: &str) -> bool {
    matches!(
        command,
        "new"
            | "open"
            | "window"
            | "split"
            | "rename"
            | "edit"
            | "close"
            | "delete"
            | "popup-new"
            | "popup-workspace-menu"
            | "popup-tab-menu"
            | "popup-restore"
            | "popup-tab-rename"
            | "popup-rename"
            | "popup-edit"
            | "refresh-status"
            | "status-click"
            | "status-menu"
            | "menu"
            | "request-close"
            | "request-rename"
            | "request-tab-rename"
            | "request-tab-close"
            | "tab-close"
            | "request-delete"
            | "snapshot"
            | "restore-arm"
            | "restore-start"
            | "restore-attached"
            | "restore"
            | "restore-discard"
            | "adopt-session"
            | "debug-path"
            | "debug-clear"
            | "help"
            | "-h"
            | "--help"
    )
}

fn shell_option(app: &App, session: &str, destination: &str) -> String {
    let shell = workspace::session_option(app, session, "@atelier_shell");
    if shell.is_empty() {
        if destination == "local" {
            "local"
        } else {
            "posix"
        }
        .into()
    } else {
        shell
    }
}
