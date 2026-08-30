#!/usr/bin/env bash

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
CLI="$ROOT/bin/tmux-atelier"

quote_sh() {
    local value=${1//\'/\'\\\'\'}
    printf "'%s'" "$value"
}

QCLI=$(quote_sh "$CLI")

set_default_option() {
    local option=$1 value=$2
    [[ -n $(tmux show-options -gqv "$option") ]] || tmux set-option -gq "$option" "$value"
}

set_default_option @atelier_tab_style default
set_default_option @atelier_tab_active_style reverse
set_default_option @atelier_workspace_active_style reverse
set_default_option @atelier_workspace_live_style bold
set_default_option @atelier_workspace_stopped_style dim
set_default_option @atelier_add_style bold
set_default_option @atelier_separator '│'
set_default_option @atelier_tab_separator '│'
set_default_option @atelier_restore prompt
set_default_option @atelier_status_sides off
set_default_option @atelier_terminal_title '#W - #S@#{@atelier_destination}'

TAB_STYLE=$(tmux show-options -gqv @atelier_tab_style)
TAB_ACTIVE_STYLE=$(tmux show-options -gqv @atelier_tab_active_style)
ADD_STYLE=$(tmux show-options -gqv @atelier_add_style)
TAB_SEPARATOR=$(tmux show-options -gqv @atelier_tab_separator)
TERMINAL_TITLE=$(tmux show-options -gqv @atelier_terminal_title)
TAB_SEPARATOR=${TAB_SEPARATOR//#/##}
TABS_FORMAT="$TAB_SEPARATOR#{W:#[range=window|#{window_index} $TAB_STYLE] #I #W #[norange default]$TAB_SEPARATOR,#[range=window|#{window_index} $TAB_ACTIVE_STYLE] #I #W #[norange default]$TAB_SEPARATOR}#[range=user|new-tab $ADD_STYLE] + #[default,norange]"
if [[ $(tmux show-options -gqv @atelier_status_sides) == on ]]; then
    STATUS_FORMAT="#[align=left range=left #{E:status-left-style}]#[push-default]#{T;=/#{status-left-length}:status-left}#[pop-default]#[norange default]#[list=on align=#{status-justify}]$TABS_FORMAT#[nolist align=right range=right #{E:status-right-style}]#[push-default]#{T;=/#{status-right-length}:status-right}#[pop-default]#[norange default]"
else
    STATUS_FORMAT="#[align=left]$TABS_FORMAT"
fi
CLICK_COMMAND="run-shell -b \"exec $QCLI status-click \\\"#{mouse_status_range}\\\" \\\"#{client_name}\\\" \\\"#{session_name}\\\"\""
MENU_COMMAND="run-shell -b \"exec $QCLI status-menu \\\"#{mouse_status_range}\\\" \\\"#{client_name}\\\"\""
REFRESH_COMMAND="run-shell -b \"$QCLI refresh-status\""
SAVE_COMMAND="run-shell -b \"$QCLI snapshot\""
RESTORE_COMMAND="run-shell -b \"$QCLI restore-start \\\"#{client_name}\\\"\""
SESSION_ADOPT_COMMAND="run-shell -b -d 0.1 \"$QCLI adopt-session \\\"#{session_name}\\\"\""
CLIENT_ADOPT_COMMAND="run-shell -b \"$QCLI adopt-session \\\"#{session_name}\\\" \\\"#{client_name}\\\"\""
TAB_MENU="display-popup -t = -e 'TMUX_ATELIER_CLIENT=#{client_name}' -E -w '45%' -h '30%' \"exec $QCLI popup-tab-menu \\\"#{window_id}\\\"\""

tmux set-option -gq @atelier_root "$ROOT"
tmux set-option -gq @atelier_cli "$CLI"
tmux set-option -gq @atelier_tabs_format "$STATUS_FORMAT"
tmux set-option -g mouse on
tmux set-option -g status 2
tmux set-option -g status-interval 5
tmux set-option -g set-titles on
tmux set-option -g set-titles-string "$TERMINAL_TITLE"
tmux set-option -g 'status-format[0]' "$STATUS_FORMAT"
tmux set-option -g 'status-format[1]' "#[range=user|new,$ADD_STYLE] + #[default,norange]"

tmux unbind-key -n MouseDown1Status
tmux bind-key -n MouseUp1Status if-shell -F '#{==:#{mouse_status_range},window}' \
    'select-window -t =' \
    "$CLICK_COMMAND"
tmux bind-key -n MouseDown3Status if-shell -F '#{m:a*,#{mouse_status_range}}' \
    "$MENU_COMMAND" \
    "$TAB_MENU"

# Replace existing native actions without choosing or changing their keys.
while IFS= read -r binding; do
    [[ $binding =~ ^bind-key[[:space:]]+(-r[[:space:]]+)?-T[[:space:]]+prefix[[:space:]]+([^[:space:]]+)[[:space:]]+(.*)$ ]] || continue
    key=${BASH_REMATCH[2]#\\}
    action=${BASH_REMATCH[3]}
    if [[ $action == new-window || $action == *"$CLI"*' window '* ]]; then
        tmux bind-key "$key" run-shell -b "exec $QCLI window \"#{session_name}\""
    elif [[ $action == 'split-window -h'* || $action == *"$CLI"*' split vertical '* ]]; then
        tmux bind-key "$key" run-shell -b "exec $QCLI split vertical \"#{pane_id}\""
    elif [[ $action == split-window* || $action == *"$CLI"*' split horizontal '* ]]; then
        tmux bind-key "$key" run-shell -b "exec $QCLI split horizontal \"#{pane_id}\""
    elif [[ $action == command-prompt* && $action == *' rename-window '* ]] ||
        [[ $action == *"$CLI"*' request-tab-rename '* ]]; then
        tmux bind-key "$key" run-shell -b "exec $QCLI request-tab-rename \"#{window_id}\" \"#{client_name}\""
    elif [[ $action == confirm-before* && $action == *kill-window* ]] ||
        [[ $action == *"$CLI"*' request-tab-close '* ]]; then
        tmux bind-key "$key" run-shell -b "exec $QCLI request-tab-close \"#{window_id}\" \"#{client_name}\""
    elif [[ $action == command-prompt* && $action == *' rename-session '* ]] ||
        [[ $action == *"$CLI"*' request-rename '* ]]; then
        tmux bind-key "$key" run-shell -b "exec $QCLI request-rename \"#{session_name}\" \"#{client_name}\""
    fi
done < <(tmux list-keys -T prefix)

"$CLI" restore-arm

for hook in session-created session-closed session-renamed client-session-changed; do
    tmux set-hook -g "${hook}[90]" "$REFRESH_COMMAND"
done

for hook in after-new-window after-split-window after-kill-pane after-select-layout \
    window-layout-changed window-renamed \
    session-window-changed window-pane-changed client-session-changed client-detached; do
    tmux set-hook -g "${hook}[91]" "$SAVE_COMMAND"
done
tmux set-hook -g 'client-attached[90]' "$RESTORE_COMMAND"
tmux set-hook -g 'session-created[91]' "$SESSION_ADOPT_COMMAND"
tmux set-hook -g 'client-attached[91]' "$CLIENT_ADOPT_COMMAND"

# tmux does not expose its initial client until configuration loading finishes.
tmux run-shell -b -d 0.5 "$QCLI restore-attached"

"$CLI" refresh-status
