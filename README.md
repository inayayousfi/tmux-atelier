# tmux-atelier

tmux-atelier turns tmux sessions into local or remote workspaces. tmux keeps shells and processes alive. OpenSSH handles remote connections. The scripts in this repository only manage creation, navigation, and the metadata needed to reopen a target.

A tmux session is a workspace, a window is a tab, and a pane is a split.

## Dependencies

Running tmux-atelier requires Bash, tmux, OpenSSH, and fzf. ShellCheck is only needed for development checks.

## Installation

The repository is self-contained. Add the following line as the last active line of your tmux configuration, replacing the path with the absolute path to the repository:

```tmux
run-shell /path/to/tmux-atelier/tmux-atelier.tmux
```

It must be loaded last so that tmux-atelier can reuse the final key bindings and status-line sections installed by the rest of the configuration. Reload the configuration or restart the tmux server. The plugin enables mouse support and installs its two-line workspace interface, but preserves the existing prefix, status-bar position, colors, and key bindings.

`.tmux.conf.example` shows the supported plugin options and loads the plugin. It does not change the prefix, status-bar position, or theme, and it is never loaded automatically.

### Oh My Tmux

tmux-atelier can be loaded from an Oh My Tmux `.tmux.conf.local`. Its first line keeps the configured Oh My Tmux `status-left` and `status-right` sections and replaces the central window list with tmux-atelier tabs. The second line contains the workspaces.

Keep the `run-shell` command as the final active tmux command in `.tmux.conf.local`, after theme options, custom bindings, and TPM plugin declarations:

```tmux
set-option -g @atelier_restore prompt
set-option -g @atelier_status_sides on
run-shell /path/to/tmux-atelier/tmux-atelier.tmux
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
```

Names may contain ASCII letters, digits, `_`, and `-`. When no name is given, tmux-atelier normalizes the final path component and adds a numeric suffix to avoid a collision.

The `created` value keeps workspaces in creation order. Older definitions without it use their file modification time. Renaming a workspace preserves this value and its position. Native tmux sessions are merged into the same line using their tmux creation time.

A definition and its live session are separate. `close` stops the session but keeps its definition. When a client is using that workspace, it switches to another live workspace first, or opens another saved workspace when necessary. `delete` removes the definition but leaves a live session alone.

Every new native tmux session is adopted as a local workspace. tmux-atelier reads the first pane's canonical working directory, reuses a saved workspace that already points there, or creates a new definition whose name comes from the directory name. If the matching workspace is already running, the temporary session is removed and its client switches to the existing workspace.

## Usage

Create and open a local workspace:

```sh
bin/tmux-atelier new local:/home/user/Projects/demo demo
```

Create and open a remote workspace:

```sh
bin/tmux-atelier new deploy@app01:/srv/app prod
```

Reopen a saved workspace whose session has stopped:

```sh
bin/tmux-atelier open prod
```

The remaining management commands are:

```sh
bin/tmux-atelier rename OLD NEW
bin/tmux-atelier edit NAME destination:path
bin/tmux-atelier close NAME
bin/tmux-atelier delete NAME
```

A target cannot be edited while its session is alive. Close the session before running `edit`. This prevents old and new targets from being mixed in one session.

## tmux Interface

The first status line shows the windows in the active session and ends with a `+` button for creating a tab on the same target. New tabs are appended after the last existing tab, even when an earlier numeric index is free.

The second status line shows saved workspaces and native tmux sessions in creation order. By default, the current workspace uses reverse video, a live session is bold, and a stopped definition is dim. Its `+` button opens the creation popup. Left-clicking a workspace opens or selects it. Right-clicking opens an FZF menu for renaming, closing, or deleting its saved definition. Right-clicking a tab opens an FZF menu for renaming or closing it. Destructive actions require confirmation.

The outer terminal title follows the active tab and workspace target using `tab - workspace@destination`. Its default format is `#W - #S@#{@atelier_destination}`, where `#W` expands to the active tab name, `#S` expands to the workspace name, and `#{@atelier_destination}` expands to `local` or the SSH destination. Set `@atelier_terminal_title` before loading the plugin to customize this tmux format string. Set `@atelier_separator` and `@atelier_tab_separator` to change the workspace and tab separators. Advanced tmux styles are available through `@atelier_workspace_active_style`, `@atelier_workspace_live_style`, `@atelier_workspace_stopped_style`, `@atelier_tab_style`, `@atelier_tab_active_style`, and `@atelier_add_style`.

The creation popup uses one machine list containing the local machine, concrete SSH aliases found through `Host` and `Include` directives in `~/.ssh/config`, and a custom SSH destination. Aliases show the user, hostname, and port resolved by `ssh -G`; custom destinations accept an alias or `user@host`.

After choosing a machine, the directory browser starts at its home directory. It can select the current directory, descend into normal or hidden directories, follow directory symlinks, move up to `/`, or return to the machine screen. Typing a relative, `~/...`, or absolute path directly into the FZF prompt creates missing directories recursively and selects the resulting path. Remote directories are created and checked through SSH. The final prompt proposes a workspace name derived from the selected directory and allows editing it.

tmux-atelier finds the keys currently assigned to native window, split, and rename actions, then replaces only their commands with workspace-aware equivalents. It does not choose the keys. With tmux's default key table, the result is:

```text
c    create a tab on the workspace target
%    split the current pane left and right on the same target
"    split the current pane top and bottom on the same target
,    rename the current tab
&    close the current tab and update the restore snapshot
$    rename the current workspace and its saved definition
```

Custom keys already assigned to these native actions are reused in the same way. The plugin does not change the prefix or navigation keys. Commands without a tmux-atelier equivalent, including the native chooser on `w`, retain their normal tmux behavior.

Tab and split commands read their target from tmux session options. Sessions that existed before the plugin was loaded keep native tmux behavior until they are attached again and adopted.

## Session Restoration

tmux-atelier continuously snapshots the topology of open managed workspaces. When a new tmux server starts, it can recreate their tabs, tab names, pane counts, exact split layouts, active tabs, and active panes. Local panes also return to their previous working directories when those directories still exist. SSH panes restart at the saved workspace root because tmux cannot reliably observe a remote shell's current directory.

Choose the startup policy before loading the plugin:

```tmux
set-option -g @atelier_restore prompt
run-shell /path/to/tmux-atelier/tmux-atelier.tmux
```

The accepted values are:

- `prompt` asks whether to restore everything or start fresh. This is the default.
- `always` restores all saved workspace topologies automatically.
- `never` starts fresh and replaces the previous snapshot.

Choosing to start fresh removes only the topology snapshot. Saved workspace definitions remain available in the status line. Closing a workspace through tmux-atelier also removes it from the next snapshot.

The snapshot is stored with private permissions at `${XDG_STATE_HOME:-$HOME/.local/state}/tmux-atelier/restore.snapshot`. It is parsed strictly as data and is never sourced as shell code.

Restoration starts new shells. It does not restore running processes, shell history, command output, unsaved editor state, or commands that were running before the tmux server stopped. Use a process-specific persistence mechanism or a plugin such as `tmux-resurrect` when that deeper restoration is required.

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

## Tests

The test suite uses an isolated tmux server, state directory, and fake SSH client. It does not touch real sessions or definitions.

```sh
./tests/run
```

Run the static checks with:

```sh
bash -n bin/tmux-atelier tmux-atelier.tmux tests/run
shellcheck bin/tmux-atelier tmux-atelier.tmux tests/run tests/fixtures/fzf tests/fixtures/ssh tests/fixtures/tmux tests/fixtures/tmux-log tests/fixtures/tmux-ui
```
