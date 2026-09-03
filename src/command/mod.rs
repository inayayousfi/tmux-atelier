mod configure;
mod lifecycle;
mod restart;
mod restore;
mod tabs;
mod ui;

use std::fs::{self, OpenOptions};
use std::ops::Deref;

use crate::app::{Command, InternalCommand};
use crate::config::Config;
use crate::interaction::{self, Interaction};
use crate::{Result, err, process, snapshot, workspace};

pub(crate) struct App {
    config: Config,
    pub(crate) interaction: Box<dyn Interaction>,
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
            interaction: interaction::from_env(),
        })
    }

    fn choose(&self, prompt: &str, options: &[String]) -> Result<Option<usize>> {
        self.interaction.choose(prompt, options)
    }

    fn input(&self, prompt: &str, initial: Option<&str>) -> Result<Option<String>> {
        self.interaction.input(prompt, initial)
    }

    fn confirm(&self, prompt: &str) -> Result<Option<bool>> {
        self.interaction.confirm(prompt)
    }

    pub(crate) fn dispatch(&self, command: Command) -> Result<()> {
        if self.restore_pending() && !allowed_during_restore(&command) {
            return Err(err(
                "workspace restoration is pending; finish the restoration prompt first",
            ));
        }
        match command {
            Command::New {
                target,
                name,
                detached,
            } => {
                let mut args = vec![target];
                if let Some(name) = name {
                    args.push(name);
                }
                if detached {
                    args.push("--detached".into());
                }
                lifecycle::new(self, &args, None)
            }
            Command::Open { name, detached } => {
                lifecycle::open(self, &name, detached.then_some("--detached"))
            }
            Command::Window { session } => tabs::window(self, session.as_deref()),
            Command::Split { orientation, pane } => {
                tabs::split(self, orientation.as_str(), pane.as_deref())
            }
            Command::Rename { old, new } => lifecycle::rename(self, &old, &new),
            Command::Edit { name, target } => lifecycle::edit(self, &name, &target, None),
            Command::Close { name } => lifecycle::close(self, &name),
            Command::Delete { name } => lifecycle::delete(self, &name),
            Command::RestartPolicy { policy, pane } => restart::set(self, policy, pane.as_deref()),
            Command::Internal { command } => self.dispatch_internal(command),
        }
    }

    fn dispatch_internal(&self, command: InternalCommand) -> Result<()> {
        match command {
            InternalCommand::Configure { root, cli } => configure::run(self, &root, &cli),
            InternalCommand::PopupNew => lifecycle::popup_new(self),
            InternalCommand::PopupWorkspaceMenu { name } => ui::popup_workspace_menu(self, &name),
            InternalCommand::PopupTabMenu { window } => ui::popup_tab_menu(self, &window),
            InternalCommand::PopupRestore { client } => {
                restore::popup_restore(self, client.as_deref())
            }
            InternalCommand::PopupTabRename { window } => tabs::popup_rename(self, &window),
            InternalCommand::PopupRename { name } => lifecycle::popup_rename(self, &name),
            InternalCommand::PopupEdit { name } => lifecycle::popup_edit(self, &name),
            InternalCommand::RefreshStatus => ui::refresh_status(self),
            InternalCommand::NavigateTab { direction, session } => {
                tabs::navigate(self, direction, &session)
            }
            InternalCommand::NavigateWorkspace {
                direction,
                session,
                client,
            } => ui::navigate_workspace(self, direction, &session, client.as_deref()),
            InternalCommand::StatusClick {
                token,
                client,
                client_id,
                session,
                window,
            } => ui::status_click(
                self,
                &token,
                Some(&client),
                &client_id,
                session.as_deref(),
                window.as_deref(),
            ),
            InternalCommand::DragStart {
                token,
                client,
                client_id,
                window,
            } => ui::drag_start(self, &token, &client, &client_id, window.as_deref()),
            InternalCommand::DragEnd {
                token,
                client,
                client_id,
                window,
            } => ui::drag_end(self, &token, &client, &client_id, window.as_deref()),
            InternalCommand::DragUpdate {
                token,
                client,
                client_id,
                window,
            } => ui::drag_update(self, &token, &client, &client_id, window.as_deref()),
            InternalCommand::DragCancel { client_id, client } => {
                ui::drag_cancel(self, &client_id, client.as_deref())
            }
            InternalCommand::CleanupDrags => ui::cleanup_drags(self),
            InternalCommand::StatusMenu {
                token,
                client,
                window,
            } => ui::status_menu(self, &token, client.as_deref(), window.as_deref()),
            InternalCommand::Menu { name, client } => ui::menu(self, &name, client.as_deref()),
            InternalCommand::RequestClose { name, client } => {
                lifecycle::request_close(self, &name, client.as_deref())
            }
            InternalCommand::RequestRename { name, client } => {
                lifecycle::request_rename(self, &name, client.as_deref())
            }
            InternalCommand::RequestTabRename { window, client } => {
                tabs::request_rename(self, &window, client.as_deref())
            }
            InternalCommand::RequestTabClose { window, client } => {
                tabs::request_close(self, &window, client.as_deref())
            }
            InternalCommand::TabClose { window } => tabs::close(self, &window),
            InternalCommand::RequestDelete { name, client } => {
                lifecycle::request_delete(self, &name, client.as_deref())
            }
            InternalCommand::ConfirmClose { name } => lifecycle::confirm_close(self, &name),
            InternalCommand::ConfirmDelete { name } => lifecycle::confirm_delete(self, &name),
            InternalCommand::ConfirmTabClose { window } => tabs::confirm_close(self, &window),
            InternalCommand::Snapshot => self.snapshot("", ""),
            InternalCommand::RestoreArm => restore::arm(self),
            InternalCommand::RestoreStart { client } => restore::start(self, client.as_deref()),
            InternalCommand::RestoreAttached => restore::attached(self),
            InternalCommand::Restore { client } => restore::run(
                self,
                client.as_deref(),
                &std::collections::HashMap::new(),
                &std::collections::HashMap::new(),
            ),
            InternalCommand::RestoreDiscard => restore::discard(self, None),
            InternalCommand::PollProcesses { generation } => restart::poll(self, &generation),
            InternalCommand::PaneRun {
                shell,
                login,
                executable,
                argv,
            } => restart::pane_run(&shell, login, &executable, &argv),
            InternalCommand::ProcessExec { executable, argv } => {
                restart::process_exec(&executable, &argv)
            }
            InternalCommand::AdoptSession { session, client } => {
                restore::adopt(self, &session, client.as_deref())
            }
            InternalCommand::DebugPath => {
                println!("{}", self.debug_log.display());
                Ok(())
            }
            InternalCommand::DebugClear => self.debug_clear(),
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

    fn restore_pending(&self) -> bool {
        process::tmux_quiet(self, &["show-options", "-gqv", "@atelier_restore_pending"]).as_deref()
            == Some("1")
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
}

fn allowed_during_restore(command: &Command) -> bool {
    matches!(
        command,
        Command::Internal {
            command: InternalCommand::Configure { .. }
                | InternalCommand::PopupRestore { .. }
                | InternalCommand::RefreshStatus
                | InternalCommand::DragCancel { .. }
                | InternalCommand::CleanupDrags
                | InternalCommand::Snapshot
                | InternalCommand::RestoreArm
                | InternalCommand::RestoreStart { .. }
                | InternalCommand::RestoreAttached
                | InternalCommand::Restore { .. }
                | InternalCommand::RestoreDiscard
                | InternalCommand::PollProcesses { .. }
                | InternalCommand::PaneRun { .. }
                | InternalCommand::ProcessExec { .. }
                | InternalCommand::AdoptSession { .. }
                | InternalCommand::DebugPath
                | InternalCommand::DebugClear
        }
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
