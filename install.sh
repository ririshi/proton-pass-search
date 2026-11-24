#!/bin/sh
# Installs proton-pass-search for the current user.
#
#   ./install.sh              binary, D-Bus service and KRunner plugin
#   ./install.sh --gnome      the same, plus the GNOME Shell search provider
#   ./install.sh --uninstall  removes everything this script installed
#
# GNOME Shell reads search providers only from system data directories, so the provider file
# needs root: the script prints the `sudo` command instead of running it.
set -eu

repo=$(cd "$(dirname "$0")" && pwd)
data="${XDG_DATA_HOME:-$HOME/.local/share}"
bin="$HOME/.local/bin/proton-pass-search"
service="$data/dbus-1/services/local.ProtonPassSearch.service"
runner="$data/krunner/dbusplugins/proton-pass-search.desktop"
gnome_app="$data/applications/local.ProtonPassSearch.desktop"
generated="$data/proton-pass-search"
provider="/usr/local/share/gnome-shell/search-providers/local.ProtonPassSearch.search-provider.ini"

dbus() {
    busctl --user call org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus "$@"
}

# The running service keeps the old binary until it exits.
stop_running() {
    pid=$(dbus GetConnectionUnixProcessID s local.ProtonPassSearch 2>/dev/null | cut -d' ' -f2) || true
    if [ -n "${pid:-}" ]; then
        kill "$pid"
    fi
}

reload() {
    dbus ReloadConfig >/dev/null
    if busctl --user status org.kde.krunner >/dev/null 2>&1; then
        kquitapp6 krunner >/dev/null 2>&1 || true
    fi
}

case "${1:-}" in
--uninstall)
    stop_running
    rm -f "$bin" "$service" "$runner" "$gnome_app"
    rm -rf "$generated"
    reload
    if [ -e "$provider" ]; then
        echo "Remove the GNOME search provider with:"
        echo "  sudo rm $provider"
    fi
    exit 0
    ;;
--gnome) gnome=yes ;;
"") gnome=no ;;
*)
    echo "usage: $0 [--gnome | --uninstall]" >&2
    exit 2
    ;;
esac

command -v pass-cli >/dev/null || {
    echo "pass-cli is not on PATH; install it from https://protonpass.github.io/pass-cli/" >&2
    exit 1
}

cargo build --release --locked --manifest-path "$repo/Cargo.toml"
stop_running
install -Dm755 "$repo/target/release/proton-pass-search" "$bin"
mkdir -p "$(dirname "$service")"
sed "s|@BIN@|$bin|" "$repo/dist/dbus/local.ProtonPassSearch.service.in" >"$service"
install -Dm644 "$repo/dist/krunner/proton-pass-search.desktop" "$runner"

if [ "$gnome" = yes ]; then
    # The provider borrows an app's name and icon, and GNOME skips it if that app is hidden.
    # Proton Pass's own app fits best; without it, install a small launcher for the web app.
    if [ -e /usr/share/applications/proton-pass.desktop ]; then
        desktop_id=proton-pass.desktop
    else
        desktop_id=local.ProtonPassSearch.desktop
        install -Dm644 "$repo/dist/gnome/$desktop_id" "$gnome_app"
    fi
    mkdir -p "$generated"
    sed "s|@DESKTOP_ID@|$desktop_id|" \
        "$repo/dist/gnome/local.ProtonPassSearch.search-provider.ini.in" \
        >"$generated/local.ProtonPassSearch.search-provider.ini"
fi

reload
echo "Installed. In KRunner, type: pp <name>"
if [ "$gnome" = yes ]; then
    echo "To finish the GNOME part, run:"
    echo "  sudo install -Dm644 $generated/local.ProtonPassSearch.search-provider.ini $provider"
    echo "then log out and back in. In the overview, type: pp <name>"
fi
