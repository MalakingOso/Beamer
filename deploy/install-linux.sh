#!/usr/bin/env bash
# Installs Beamer to ~/.local/bin, never /usr/bin. A .deb install needs root
# to write /usr/bin, which blocks the in-app self-updater from ever swapping
# the binary in place. See agent_docs/running_on_bearcave.md.
#
# The binary is the whole app: stylesheet, fonts, icon and the GNOME
# extension's files are all compiled in, so a self-update (which swaps only
# the binary) always leaves a complete install.
#
# Usage: run from a checkout after `dx build --release`. If you answer yes to
# the sync_server prompt below, this script builds that binary itself.
#
# Idempotent: safe to re-run, e.g. to pick up a freshly rebuilt binary, or to
# add the sync server later by re-running and answering yes.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

bin_dir="$HOME/.local/bin"

# --- Locate the build output --------------------------------------------------

exe="$(find target -type f -name beamer -not -path '*/build/*' -not -path '*/incremental/*' \
            -printf '%T@ %p\n' 2>/dev/null | sort -rn | head -n1 | cut -d' ' -f2-)"

if [ -z "$exe" ]; then
    echo "error: no 'beamer' binary found under target/." >&2
    echo "Run 'dx build --release' first." >&2
    exit 1
fi

echo "Found binary: $exe"

# --- Install beamer ----------------------------------------------------------

mkdir -p "$bin_dir"
install -m755 "$exe" "$bin_dir/beamer"
echo "Installed beamer to $bin_dir/beamer"

# Older installs put hashed asset files and a copy of the GNOME extension
# sources beside the binary. Nothing reads either any more; clear them out.
for stale in "$HOME/.local/lib/Beamer/assets" "$HOME/.local/share/beamer/extension"; do
    if [ -e "$stale" ]; then
        rm -rf "$stale"
        echo "Removed obsolete $stale"
    fi
done
rmdir "$HOME/.local/lib/Beamer" "$HOME/.local/share/beamer" 2>/dev/null || true

# --- Optional: sync_server ---------------------------------------------------
#
# Only one machine should run this — see deploy/beamer-sync.service.

want_sync="${BEAMER_INSTALL_SYNC_SERVER:-}"
if [ -z "$want_sync" ]; then
    read -r -p "Install the sync server (background note-sync service) on this machine? [y/N] " want_sync || true
fi

case "$want_sync" in
    y|Y|yes|YES|1)
        echo "Building sync_server..."
        cargo build --release --locked --bin sync_server
        install -m755 "target/release/sync_server" "$bin_dir/sync_server"
        echo "Installed sync_server to $bin_dir/sync_server"

        unit_dir="$HOME/.config/systemd/user"
        mkdir -p "$unit_dir"
        # Only the binary path changes; the bind address and --config-dir
        # stay exactly as deploy/beamer-sync.service defines them, so that
        # file stays the single source of truth for the rest of the unit.
        sed 's#/usr/bin/sync_server#%h/.local/bin/sync_server#' \
            "deploy/beamer-sync.service" > "$unit_dir/beamer-sync.service"
        systemctl --user daemon-reload
        echo "Installed unit to $unit_dir/beamer-sync.service"

        echo
        echo "Not enabled automatically. A user unit of the same name overrides"
        echo "/usr/lib/systemd/user/beamer-sync.service, so:"
        echo "  - if beamer-sync was never enabled:  systemctl --user enable --now beamer-sync"
        echo "  - if it's already running (old .deb unit): systemctl --user restart beamer-sync"
        ;;
    *)
        echo "Skipping sync_server."
        ;;
esac

echo
echo "Done. Launch with: beamer"
echo "(make sure ~/.local/bin precedes /usr/bin on PATH)"
