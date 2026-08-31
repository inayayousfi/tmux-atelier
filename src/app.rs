use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

use crate::command::App;
use crate::Result;

#[derive(Parser)]
#[command(
    name = "tmux-atelier",
    version,
    about = "Turn tmux sessions into local or remote workspaces"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    New {
        target: String,
        #[arg(allow_hyphen_values = true)]
        name: Option<String>,
        #[arg(long)]
        detached: bool,
    },
    Open {
        #[arg(allow_hyphen_values = true)]
        name: String,
        #[arg(long)]
        detached: bool,
    },
    Window {
        #[arg(allow_hyphen_values = true)]
        session: Option<String>,
    },
    Split {
        orientation: Orientation,
        #[arg(allow_hyphen_values = true)]
        pane: Option<String>,
    },
    Rename {
        #[arg(allow_hyphen_values = true)]
        old: String,
        #[arg(allow_hyphen_values = true)]
        new: String,
    },
    Edit {
        #[arg(allow_hyphen_values = true)]
        name: String,
        target: String,
    },
    Close {
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
    Delete {
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
    #[command(hide = true)]
    Internal {
        #[command(subcommand)]
        command: InternalCommand,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Orientation {
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Direction {
    Next,
    Previous,
}

impl Orientation {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Vertical => "vertical",
            Self::Horizontal => "horizontal",
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum InternalCommand {
    Configure {
        root: PathBuf,
        cli: PathBuf,
    },
    PopupNew,
    PopupWorkspaceMenu {
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
    PopupTabMenu {
        window: String,
    },
    PopupRestore {
        client: Option<String>,
    },
    PopupTabRename {
        window: String,
    },
    PopupRename {
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
    PopupEdit {
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
    RefreshStatus,
    NavigateTab {
        direction: Direction,
        session: String,
    },
    NavigateWorkspace {
        direction: Direction,
        session: String,
        client: Option<String>,
    },
    StatusClick {
        token: String,
        client: Option<String>,
        session: Option<String>,
    },
    StatusMenu {
        token: String,
        client: Option<String>,
        window: Option<String>,
    },
    Menu {
        #[arg(allow_hyphen_values = true)]
        name: String,
        client: Option<String>,
    },
    RequestClose {
        #[arg(allow_hyphen_values = true)]
        name: String,
        client: Option<String>,
    },
    RequestRename {
        #[arg(allow_hyphen_values = true)]
        name: String,
        client: Option<String>,
    },
    RequestTabRename {
        window: String,
        client: Option<String>,
    },
    RequestTabClose {
        window: String,
        client: Option<String>,
    },
    TabClose {
        window: String,
    },
    RequestDelete {
        #[arg(allow_hyphen_values = true)]
        name: String,
        client: Option<String>,
    },
    ConfirmClose {
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
    ConfirmDelete {
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
    ConfirmTabClose {
        window: String,
    },
    Snapshot,
    RestoreArm,
    RestoreStart {
        client: Option<String>,
    },
    RestoreAttached,
    Restore {
        client: Option<String>,
    },
    RestoreDiscard,
    AdoptSession {
        #[arg(allow_hyphen_values = true)]
        session: String,
        client: Option<String>,
    },
    DebugPath,
    DebugClear,
}

pub fn run() -> Result<()> {
    App::from_env()?.dispatch(Cli::parse().command)
}
