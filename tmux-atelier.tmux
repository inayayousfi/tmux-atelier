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

TAB_STYLE=$(tmux show-options -gqv @atelier_tab_style)
TAB_ACTIVE_STYLE=$(tmux show-options -gqv @atelier_tab_active_style)
ADD_STYLE=$(tmux show-options -gqv @atelier_add_style)
TABS_FORMAT="#[align=left]#{W:#[range=window|#{window_index} $TAB_STYLE] #I #W #[norange default] ,#[range=window|#{window_index} $TAB_ACTIVE_STYLE] #I #W #[norange default] }#[range=user|new-tab $ADD_STYLE] + #[default,norange]"
CLICK_COMMAND="run-shell -b \"exec $QCLI status-click \\\"#{mouse_status_range}\\\" \\\"#{client_name}\\\" \\\"#{session_name}\\\"\""
MENU_COMMAND="run-shell -b \"exec $QCLI status-menu \\\"#{mouse_status_range}\\\" \\\"#{client_name}\\\"\""
REFRESH_COMMAND="run-shell -b \"$QCLI refresh-status\""
TAB_MENU="display-popup -t = -e 'TMUX_ATELIER_CLIENT=#{client_name}' -E -w '45%' -h '30%' \"exec $QCLI popup-tab-menu \\\"#{window_id}\\\"\""

tmux set-option -gq @atelier_root "$ROOT"
tmux set-option -gq @atelier_cli "$CLI"
tmux set-option -gq @atelier_tabs_format "$TABS_FORMAT"
tmux set-option -g mouse on
tmux set-option -g status 2
tmux set-option -g status-interval 5
tmux set-option -g 'status-format[0]' "$TABS_FORMAT"
tmux set-option -g 'status-format[1]' "#[range=user|new,$ADD_STYLE] + #[default,norange]"

tmux bind-key -n MouseDown1Status if-shell -F '#{==:#{mouse_status_range},window}' \
    'select-window -t =' \
    "$CLICK_COMMAND"
tmux bind-key -n MouseDown3Status if-shell -F '#{m:a*,#{mouse_status_range}}' \
    "$MENU_COMMAND" \
    "$TAB_MENU"

for hook in session-created session-closed session-renamed client-session-changed; do
    tmux set-hook -g "${hook}[90]" "$REFRESH_COMMAND"
done

"$CLI" refresh-status
