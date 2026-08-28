#!/usr/bin/env bash

set -euo pipefail

ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
CLI="$ROOT/bin/tmux-atelier"

quote_sh() {
    local value=${1//\'/\'\\\'\'}
    printf "'%s'" "$value"
}

QCLI=$(quote_sh "$CLI")
TABS_FORMAT='#[align=left,fg=default,bg=default]#{W:#[range=window|#{window_index}] #I #W #[norange default] ,#[range=window|#{window_index} reverse] #I #W #[norange default] }'
CLICK_COMMAND=$(quote_sh "$QCLI status-click '#{mouse_status_range}' '#{client_name}'")
MENU_COMMAND=$(quote_sh "$QCLI status-menu '#{mouse_status_range}' '#{client_name}'")
REFRESH_COMMAND=$(quote_sh "$QCLI refresh-status")

tmux set-option -gq @atelier_root "$ROOT"
tmux set-option -gq @atelier_cli "$CLI"
tmux set-option -gq @atelier_tabs_format "$TABS_FORMAT"
tmux set-option -g prefix C-a
tmux unbind-key C-b 2>/dev/null || true
tmux bind-key C-a send-prefix
tmux set-option -g mouse on
tmux set-option -g status 2
tmux set-option -g status-position bottom
tmux set-option -g status-interval 5
tmux set-option -g status-style default
tmux set-option -g 'status-format[0]' "$TABS_FORMAT"
tmux set-option -g 'status-format[1]' '#[range=user|new,bold] + #[default,norange]'

tmux bind-key a display-popup -E -w '70%' -h '60%' "$QCLI picker"
tmux bind-key n display-popup -E -w '70%' -h '60%' "$QCLI popup-new"
tmux bind-key t run-shell -b "$QCLI window '#{session_name}'"
tmux bind-key + run-shell -b "$QCLI split vertical '#{pane_id}'"
tmux bind-key - run-shell -b "$QCLI split horizontal '#{pane_id}'"

tmux bind-key -n MouseDown1Status if-shell -F '#{==:#{mouse_status_range},window}' \
    'select-window -t =' \
    "run-shell -b $CLICK_COMMAND"
tmux bind-key -n MouseDown3Status if-shell -F '#{m:a*,#{mouse_status_range}}' \
    "run-shell -b $MENU_COMMAND" \
    'display-message "No workspace menu here"'

for hook in session-created session-closed session-renamed client-session-changed; do
    tmux set-hook -g "${hook}[90]" "run-shell -b $REFRESH_COMMAND"
done

"$CLI" refresh-status
