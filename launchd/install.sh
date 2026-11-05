#!/bin/sh
# Install, remove or show the launchd agent that starts tach-sender.py at login.
#
#   launchd/install.sh            install (or reinstall) and start the agent
#   launchd/install.sh uninstall  stop and remove the agent
#   launchd/install.sh status     show whether the agent runs
set -eu

LABEL=com.peterichardson.tach-sender
TACH_DIR=$(cd "$(dirname "$0")/.." && pwd)
TEMPLATE="$TACH_DIR/launchd/$LABEL.plist.in"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN="gui/$(id -u)"

stop_agent() {
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
    # bootout returns before the job is fully gone; bootstrap fails until it is.
    while launchctl print "$DOMAIN/$LABEL" >/dev/null 2>&1; do sleep 0.2; done
}

case "${1:-install}" in
install)
    if [ ! -x "$TACH_DIR/.venv/bin/python" ]; then
        echo "No .venv. Run 'uv sync' in $TACH_DIR first." >&2
        exit 1
    fi
    if ! MACMON=$(command -v macmon); then
        echo "macmon not found. Install it: brew install macmon" >&2
        exit 1
    fi
    mkdir -p "$HOME/Library/LaunchAgents" "$HOME/Library/Logs"
    sed -e "s|__TACH_DIR__|$TACH_DIR|g" \
        -e "s|__HOME__|$HOME|g" \
        -e "s|__MACMON_DIR__|$(dirname "$MACMON")|g" \
        "$TEMPLATE" > "$PLIST"
    plutil -lint "$PLIST" >/dev/null
    stop_agent
    launchctl bootstrap "$DOMAIN" "$PLIST"
    echo "Installed $PLIST"
    echo "Log: $HOME/Library/Logs/tach-sender.log"
    ;;
uninstall)
    stop_agent
    rm -f "$PLIST"
    echo "Removed $LABEL"
    ;;
status)
    tab=$(printf '\t')
    if launchctl print "$DOMAIN/$LABEL" 2>/dev/null | grep -E "^${tab}(state|pid|last exit code) ="; then
        :
    else
        echo "$LABEL is not loaded"
    fi
    ;;
*)
    echo "usage: $0 [install|uninstall|status]" >&2
    exit 2
    ;;
esac
