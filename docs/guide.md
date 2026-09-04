# tmux-atelier guide

tmux-atelier turns tmux sessions into local or remote workspaces. tmux keeps shells and processes alive. OpenSSH handles remote connections. The Rust binary manages creation, navigation, restoration, and the metadata needed to reopen a target. A small shell adapter connects it to tmux.

A tmux session is a workspace, a window is a tab, and a pane is a split.

## Dependencies

Running tmux-atelier requires tmux. OpenSSH is also required when using remote workspaces. The plugin adapter is POSIX `sh` and has no Bash or fzf runtime dependency.

Building from source requires Rust 1.89 or newer. Bash and ShellCheck are used for the installer and development checks.

## Installation

Install the latest binary and tmux adapter under `${XDG_CONFIG_HOME:-$HOME/.config}/tmux/tmux-atelier`:

```sh
curl -fsSL https://raw.githubusercontent.com/inayayousfi/tmux-atelier/main/install.sh | bash
```

The installer detects Linux x86-64, Linux ARM64, or macOS Apple Silicon, verifies the release checksum, and replaces an older installation atomically. It does not edit tmux configuration. Add the line it prints as the last active line of `${XDG_CONFIG_HOME:-$HOME/.config}/tmux/tmux.conf`:

```tmux
run-shell ~/.config/tmux/tmux-atelier/tmux-atelier.tmux
```

To install elsewhere, pass `--install-dir` (or `-d`) to the installer. When piping the script to Bash, pass installer arguments after `-s --`:

```sh
curl -fsSL https://raw.githubusercontent.com/inayayousfi/tmux-atelier/main/install.sh | bash -s -- --install-dir "$HOME/path/to/tmux-atelier"
```

Use the exact absolute path printed by the installer when `XDG_CONFIG_HOME` is set. The adapter must be loaded last so tmux-atelier can reuse the final key bindings and status-line sections installed by the rest of the configuration. Reload the configuration or restart the tmux server. The plugin enables mouse support and installs its two-line workspace interface, but preserves the existing prefix, status-bar position, colors, and key bindings.

To run directly from a source checkout, build the binary first with `cargo build`. The adapter finds `target/debug/tmux-atelier` when an installed `bin/tmux-atelier` is absent.

`.tmux.conf.example` shows the supported plugin options and loads the plugin. It does not change the prefix, status-bar position, or theme, and it is never loaded automatically.

### Oh My Tmux

tmux-atelier can be loaded from an Oh My Tmux `.tmux.conf.local`. Its first line keeps the configured Oh My Tmux `status-left` and `status-right` sections and replaces the central window list with tmux-atelier tabs. The second line contains the workspaces.

Keep the `run-shell` command as the final active tmux command in `.tmux.conf.local`, after theme options, custom bindings, and Tmux Plugin Manager declarations:

```tmux
set-option -g @atelier_restore prompt
set-option -g @atelier_status_sides on
run-shell ~/.config/tmux/tmux-atelier/tmux-atelier.tmux
```

`@atelier_status_sides` is disabled by default so a regular tmux configuration does not show tmux's default session name, pane title, and date around the tabs.

`oh-my-tmux.conf.local.example` is a minimal integration example based on the development configuration: `C-a` is the only prefix, the status bar stays at the bottom, tabs are left aligned, and mouse support is enabled by tmux-atelier. The visible `OMT` label distinguishes the Oh My Tmux status sections from tmux's defaults.

Run the integration without changing the current tmux server or saved workspaces:

```sh
./tests/oh-my-tmux
```

The command downloads Oh My Tmux into `.tmp/oh-my-tmux` on its first run, generates a local configuration with the current repository path, and starts a disposable server on the `tmux-atelier-oh-my-tmux` socket. Exit that test session normally to return to the existing tmux session.

## Targets

A target uses the format `destination:path`:

```text
local:/home/user/Projects/tmux-atelier
app01:/srv/app
deploy@app01:/srv/app
```

`local` means the local machine. Every other destination is passed to OpenSSH, so it may be a host, an alias from `~/.ssh/config`, or a `user@host` destination.

Destinations may contain ASCII letters, digits, `.`, `_`, `-`, and `@`. Define an alias in `~/.ssh/config` for a raw IPv6 address because `:` separates the destination from the path.

Remote commands use OpenSSH connection sharing with `ControlMaster=auto` and a one-minute `ControlPersist`. Private control sockets live under the tmux-atelier state directory. Authentication, host verification, keys, agents, ports, jump hosts, and other connection settings remain under OpenSSH control; tmux-atelier never reads or stores credentials.

Paths may contain spaces, apostrophes, and `=` signs. They may not contain newlines. A local path must be an existing directory. The remote machine checks a remote path when SSH connects.

## Saved Workspaces

Definitions are stored under:

```text
${XDG_STATE_HOME:-$HOME/.local/state}/tmux-atelier/workspaces/
```

Each file has the workspace name and is parsed as data, never executed as shell code:

```ini
name=prod
destination=deploy@app01
path=/srv/app
created=1787911200
shell=posix
```

Names may contain ASCII letters, digits, `_`, and `-`. When no name is given, tmux-atelier normalizes the final path component and adds a numeric suffix to avoid a collision.

The `created` value supplies the initial workspace order. Older definitions without it use their file modification time. After a workspace is dragged to a new position, the complete order is stored in `${XDG_STATE_HOME:-$HOME/.local/state}/tmux-atelier/workspace.order`. Renaming a workspace preserves both its creation time and position. New saved workspaces and native tmux sessions are appended in creation order.

A definition and its live session are separate. `close` stops the session but keeps its definition. When a client is using that workspace, it switches to another live workspace first, or opens another saved workspace when necessary. `delete` stops a running session with the same safety checks and then removes the definition.

Every new native tmux session is adopted as a local workspace. tmux-atelier reads the first pane's canonical working directory, reuses a saved workspace that already points there, or creates a new definition whose name comes from the directory name. If the matching workspace is already running, the temporary session is removed and its client switches to the existing workspace.

## Usage

Create and open a local workspace:

```sh
~/.config/tmux/tmux-atelier/bin/tmux-atelier new local:/home/user/Projects/demo demo
```

Create and open a remote workspace:

```sh
~/.config/tmux/tmux-atelier/bin/tmux-atelier new deploy@app01:/srv/app prod
```

Reopen a saved workspace whose session has stopped:

```sh
~/.config/tmux/tmux-atelier/bin/tmux-atelier open prod
```

The remaining management commands are:

```sh
~/.config/tmux/tmux-atelier/bin/tmux-atelier rename OLD NEW
~/.config/tmux/tmux-atelier/bin/tmux-atelier edit NAME destination:path
~/.config/tmux/tmux-atelier/bin/tmux-atelier close NAME
~/.config/tmux/tmux-atelier/bin/tmux-atelier delete NAME
```

Editing a live workspace updates the saved definition and the session metadata used by new tabs and splits. Existing panes keep running at their current locations.

## tmux Interface

The first status line shows the windows in the active session and ends with a `+` button for creating a tab on the same target. New tabs are appended after the last existing tab, even when an earlier numeric index is free.

The second status line shows saved workspaces and native tmux sessions in their saved order. By default, the current workspace uses reverse video, a live session is bold, and a stopped definition is dim. Its `+` button opens the creation popup. Left-clicking a workspace opens or selects it. Dragging a workspace onto another workspace inserts it at that workspace's former position. While dragging, the source uses the status theme with `reverse,dim`, and a valid landing target uses the existing active-item style with an underline. These signals contain no fixed colors, so they follow the current tmux theme. Right-clicking opens a menu for editing, stopping, or deleting it. Stopped workspaces also offer an explicit open action. Dragging a tab onto another tab uses the same source and target signals and swaps the real tmux windows through existing numeric positions. The dragged windows take different indexes, but the session's index set does not grow or shift; the saved topology restores that order after a restart. Right-clicking a tab opens a menu for renaming or closing it. Destructive actions require confirmation.

The outer terminal title follows the active tab and workspace target using `tab - workspace@destination`. Its default format is `#W - #S@#{@atelier_destination}`, where `#W` expands to the active tab name, `#S` expands to the workspace name, and `#{@atelier_destination}` expands to `local` or the SSH destination. Set `@atelier_terminal_title` before loading the plugin to customize this tmux format string. Set `@atelier_separator` and `@atelier_tab_separator` to change the separators shown before, between, and after workspaces and tabs. Advanced tmux styles are available through `@atelier_workspace_active_style`, `@atelier_workspace_live_style`, `@atelier_workspace_stopped_style`, `@atelier_tab_style`, `@atelier_tab_active_style`, and `@atelier_add_style`. `@atelier_drag_source_style` replaces an item's style while it is dragged and defaults to `default,reverse,dim`. `@atelier_drag_target_style` is added to the relevant active-item style at a valid landing position and defaults to `underscore`.

The creation popup uses one machine list containing the local machine, concrete SSH aliases found through `Host` and `Include` directives in `~/.ssh/config`, SSH destinations recovered from common shell history files, and a custom SSH destination. It recognizes `user@host` destination operands in Bash, Zsh, Fish, POSIX/Ksh, and Nushell text histories. Aliases show the user, hostname, and port resolved by `ssh -G`; custom destinations accept an alias or `user@host`.

Recovered and successfully used custom SSH destinations are saved with private permissions in `${XDG_STATE_HOME:-$HOME/.local/state}/tmux-atelier/ssh-destinations`, so they remain available after shell history is cleared.

After choosing a machine, the directory browser starts at its home directory. It can select the current directory, descend into normal or hidden directories, follow directory symlinks, move up to `/`, or return to the machine screen. The `Enter a path` action accepts a relative, `~/...`, or absolute path, creates missing directories recursively, and selects the result. Remote directories are created and checked through SSH. The final prompt proposes a workspace name derived from the selected directory and allows editing it.

tmux-atelier finds the keys currently assigned to native window, split, and rename actions, then replaces only their commands with workspace-aware equivalents. It does not choose the keys. With tmux's default key table, the result is:

```text
c    create a tab on the workspace target
n    select the next tab
p    select the previous tab
N    create a workspace
%    split the current pane left and right on the same target
"    split the current pane top and bottom on the same target
,    rename the current tab
&    close the current tab and update the restore snapshot
$    rename the current workspace and its saved definition
)    select the next workspace
(    select the previous workspace
```

Custom keys already assigned to these native actions are reused in the same way. Workspace navigation follows the full creation-ordered Atelier list, wraps at either end, and opens a stopped saved workspace when selected. Both status rows scroll with their active item when the terminal is too narrow to show every tab or workspace. The plugin does not change the prefix. Commands without a tmux-atelier equivalent, including the native chooser on `w`, retain their normal tmux behavior.

The new-workspace shortcut defaults to `N`. Set `@atelier_new_workspace_key` before loading the plugin to choose another prefix key, or set it to `off` to disable the shortcut. Atelier owns the popup command, so the configuration only needs the key:

```tmux
set-option -g @atelier_new_workspace_key N
```

Tab and split commands read their target from tmux session options. Sessions that existed before the plugin was loaded keep native tmux behavior until they are attached again and adopted.

## Session Restoration

tmux-atelier continuously snapshots the topology of open managed workspaces. When a new tmux server starts, it can recreate their tabs, tab names, pane counts, exact split layouts, active tabs, and active panes. Local panes return to their previous working directories when those directories still exist and fall back to the workspace root after a saved directory is deleted. SSH panes restart at the saved workspace root because tmux cannot reliably observe a remote shell's current directory.

Choose the startup policy before loading the plugin:

```tmux
set-option -g @atelier_restore prompt
run-shell ~/.config/tmux/tmux-atelier/tmux-atelier.tmux
```

The accepted values are:

- `prompt` asks whether to restore changed workspace topologies or start fresh. This is the default.
- `always` restores missing workspaces automatically. It still asks before replacing a live workspace whose topology differs.
- `never` starts fresh and replaces the previous snapshot.

At startup, Atelier compares each saved workspace with its live tmux session. The comparison covers tab counts, indexes, explicit names, automatic renaming, pane counts, split layouts, active tabs and panes, and local working directories. Names generated while automatic renaming is on are not stable topology and are ignored. Tmux pane IDs, layout checksums, and whether the workspace is currently selected are not topology differences. Deleted workspace definitions and live workspaces absent from the snapshot are ignored.

The prompt is skipped when every restorable workspace matches. Missing workspaces are recreated. A mismatched live workspace is staged under a temporary session name first, then replaced only after explicit confirmation; the warning lists every session whose current shells will stop. Atelier rechecks the exact user-owned topology immediately before replacement, so a pane, path, explicit name, active selection, or layout change after the prompt cancels the operation. Declining the warning cancels the complete restoration and starts fresh from the current tmux state.

Before staging a replacement, Atelier stores its transaction generation plus original, temporary, and backup names on the original tmux session. A server-wide phase records one decision for the complete generation. Restore execution is serialized, and a new generation cannot overwrite an unfinished one. If restoration is interrupted before commit, the next startup restores every marked original. After commit, cleanup only deletes remaining backups and never rolls a replacement back; the committed phase remains authoritative until restore-control options are normalized. Temporary and newly created sessions receive the generation in the same tmux command queue that creates them, so cleanup never deletes an unrelated session that claimed the same candidate name. Foreground commands are not started in temporary sessions.

While a restoration decision is pending, workspace and tab actions provided by Atelier are blocked so a status-bar click cannot create a one-tab workspace ahead of restoration. Native tmux commands remain available for recovery.

Choosing to start fresh discards the previous topology and snapshots the current live state. Saved workspace definitions remain available in the status line. Closing a workspace through tmux-atelier also removes it from the next snapshot.

The snapshot is stored with private permissions at `${XDG_STATE_HOME:-$HOME/.local/state}/tmux-atelier/restore.snapshot`. It is parsed strictly as data and is never sourced as shell code.

### Foreground Processes

On Linux, Atelier polls local panes and can restart an approved foreground command with its complete UTF-8 argument list and working directory. Empty arguments are preserved. A command containing a non-UTF-8 argument cannot be captured in the current snapshot format. Environment variables are never read or saved. Shell assignments such as `TOKEN=value command` therefore disappear, while a value passed as a command-line argument remains part of the private snapshot.

Polling defaults to every five seconds, and an automatically captured process must have run for at least five seconds. Set either option before loading the plugin:

```tmux
set-option -g @atelier_restart_interval 5
set-option -g @atelier_restart_min_runtime 5
```

Set `@atelier_restart_interval` to `off` to disable process capture and restoration. Polling rewrites the snapshot only when pane state changes. macOS and remote panes currently restore topology without foreground processes.

Automatic capture checks the root foreground command against this default executable-basename denylist. Helper processes do not block their parent command: for example, OpenCode remains eligible when it starts a denylisted `node` MCP server. A root command that is itself `node` remains excluded.

```text
awk bash basename cat chmod chown cmake cp curl cut date dd diff dirname du
echo env false fd find fish fzf git go grep head install kill less ln ls
make man mkdir mv ninja node npm pacman pnpm printf pwd python python3
readlink realpath rg rm rmdir rsync ruby scp sed sh sleep sort ssh stat
tail tar tee test tmux touch tr true uname uniq wc wget xargs zsh
```

Replace `@atelier_restart_denylist` with a whitespace-separated list to customize it. A pane can override automatic behavior:

```sh
tmux-atelier restart-policy always
tmux-atelier restart-policy never %12
tmux-atelier restart-policy auto %12
```

The tab menu exposes the same `auto`, `always`, and `never` choices for its active pane. `always` bypasses the denylist and minimum runtime. `never` removes the saved process recipe. Pipelines and other foreground groups whose original shell syntax cannot be reconstructed are not captured.

Before changing any pane, restoration compares every saved argv with its live foreground process and prepares the complete restart plan. An idle pane restarts the saved command in place. Replacing a different live process requires explicit confirmation, including under `@atelier_restore always`. Prompts show the program and pane but never its arguments.

Bash, zsh, and fish commands start through the captured shell and login mode so current profile files apply. The private launcher uses the saved executable path for lookup and assigns the captured `argv[0]` separately, including an empty or custom value. Immediately before changing any pane, Atelier rechecks every planned live pane ID, foreground process, and workspace topology; one stale value at that point skips the complete process plan. It also rechecks each live pane immediately before its own restart. Tmux has no atomic compare-and-restart operation, so a later pane can still change after its final check while an earlier pane is restarting; that later launch remains best effort rather than all-or-none. Tmux accepting the pane restart counts as a successful launch; Atelier does not impose a startup deadline or claim that the command remains running. A rejected launch leaves restoration complete, reports a warning, and does not block launches in other panes. When a started command finishes, the pane returns to its captured shell. Program output and errors remain visible; Atelier also prints a termination signal or nonzero exit status. An unsupported shell is an intentional successful fallback: it reports a warning and returns to the captured shell without starting the saved command.

Process restart does not restore process memory, shell history, command output from before shutdown, unsaved editor state, pipelines, redirections, aliases, or functions. Applications such as Neovim and OpenCode still need their own persistence when internal state matters.

## Native Recovery

Running processes belong to tmux and OpenSSH, not tmux-atelier. Removing the repository or breaking a script does not stop existing panes.

Native commands remain available through the configured tmux prefix followed by `:`. Useful recovery commands include:

```sh
tmux ls
tmux attach-session -t NAME
tmux switch-client -t NAME
tmux new-window
tmux split-window
```

Without the scripts, `new-window` and `split-window` still work, but they no longer reconstruct a saved remote target. Metadata for a managed live session remains available through tmux:

```sh
tmux show-options -t NAME: @atelier_destination
tmux show-options -t NAME: @atelier_path
```

## Development

The Rust implementation is divided by responsibility: typed CLI commands, interaction, tmux configuration, workspace lifecycle, tabs and panes, restoration and adoption, persistence, process adapters, and the target wizard. The application context carries configured state paths and adapters through those modules.

Build and run the Rust checks with:

```sh
cargo build
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

## Tests

The Rust process suite uses isolated tmux servers, state directories, a fake SSH client, and scripted interaction. It does not touch real sessions or definitions. The installer remains covered at its shell boundary.

```sh
cargo test --all-targets
./tests/install
```

Run the static checks with:

```sh
bash -n install.sh tests/install tests/fixtures/ssh tests/fixtures/tmux
sh -n tmux-atelier.tmux
shellcheck install.sh tmux-atelier.tmux tests/install tests/fixtures/ssh tests/fixtures/tmux
```
