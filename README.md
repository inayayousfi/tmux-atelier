# tmux-atelier

tmux-atelier turns tmux sessions into local or remote workspaces. tmux keeps shells and processes alive. OpenSSH handles remote connections. The scripts in this repository only manage creation, navigation, and the metadata needed to reopen a target.

A tmux session is a workspace, a window is a tab, and a pane is a split.

## Dependencies

Running tmux-atelier requires Bash, tmux, OpenSSH, and fzf. ShellCheck is only needed for development checks.

## Installation

The repository is self-contained. Add the following line to your tmux configuration, replacing the path with the absolute path to the repository:

```tmux
run-shell /path/to/tmux-atelier/tmux-atelier.tmux
```

Reload the configuration or restart the tmux server. The plugin enables mouse support and installs its two-line workspace interface, but preserves the existing prefix, status-bar position, colors, and key bindings.

For the complete HERDR setup, copy the relevant settings from `.tmux.conf.example` instead. The example sets `Ctrl-a` as the prefix, places the status bar at the bottom, defines a color theme and key bindings, and loads the plugin. It is documentation only and is never loaded automatically.

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

A definition and its live session are separate. `close` stops the session but keeps its definition. `delete` removes the definition but leaves a live session alone.

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

Set `@atelier_workspace_active_style`, `@atelier_workspace_live_style`, `@atelier_workspace_stopped_style`, `@atelier_tab_style`, `@atelier_tab_active_style`, and `@atelier_add_style` before loading the plugin to customize those elements. `.tmux.conf.example` shows one complete theme.

The creation popup uses one machine list containing the local machine, concrete SSH aliases found through `Host` and `Include` directives in `~/.ssh/config`, and a custom SSH destination. Aliases show the user, hostname, and port resolved by `ssh -G`; custom destinations accept an alias or `user@host`.

After choosing a machine, the directory browser starts at its home directory. It can select the current directory, descend into normal or hidden directories, follow directory symlinks, move up to `/`, or return to the machine screen. Typing a relative, `~/...`, or absolute path directly into the FZF prompt creates missing directories recursively and selects the resulting path. Remote directories are created and checked through SSH. The final prompt proposes a workspace name derived from the selected directory and allows editing it.

The optional key bindings in `.tmux.conf.example` follow HERDR while preserving tmux's native workspace tree on `Ctrl-a w`:

```text
Ctrl-a w    open the native tmux workspace tree
Ctrl-a n    create a workspace
Ctrl-a t    create a tab on the same target
Ctrl-a r    reload tmux-atelier
Ctrl-a =    create a vertical split on the same target
Ctrl-a -    create a horizontal split on the same target
Ctrl-a h    select the pane on the left
Ctrl-a j    select the pane below
Ctrl-a k    select the pane above
Ctrl-a l    select the pane on the right
```

Tab and split commands read their target from tmux session options. A session created without tmux-atelier keeps native tmux behavior.

## Native Recovery

Running processes belong to tmux and OpenSSH, not tmux-atelier. Removing the repository or breaking a script does not stop existing panes.

Native commands remain available through the configured tmux prefix followed by `:`. With `.tmux.conf.example`, that is `Ctrl-a :`. Useful recovery commands include:

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
