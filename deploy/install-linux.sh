#!/usr/bin/env bash
# Installs Beamer to ~/.local/{bin,lib}, never /usr/bin. A .deb install needs
# root to write /usr/bin, which blocks the in-app self-updater from ever
# swapping the binary in place. See agent_docs/running_on_bearcave.md.
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
lib_dir="$HOME/.local/lib/Beamer"

# --- Locate the dx build output ---------------------------------------------
#
# A raw `dx build` lays assets flat next to the binary (target/dx/beamer/
# release/linux/app/{beamer,assets}/) rather than in the bin/+lib/Beamer/
# layout a bundled install gets — dioxus-asset-resolver's get_asset_root()
# falls back to "assets beside the exe" when no lib/ dir exists two levels
# up. Require that sibling assets/ directory so a plain `cargo build`
# binary (which has none) is rejected instead of installed unstyled.

exe=""
assets_src=""
while IFS= read -r candidate; do
    candidate_assets="$(dirname "$candidate")/assets"
    if [ -d "$candidate_assets" ] && [ -n "$(ls -A "$candidate_assets" 2>/dev/null)" ]; then
        exe="$candidate"
        assets_src="$candidate_assets"
        break
    fi
done < <(find target -type f -name beamer -not -path '*/build/*' -not -path '*/incremental/*' \
              -printf '%T@ %p\n' 2>/dev/null | sort -rn | cut -d' ' -f2-)

if [ -z "$exe" ]; then
    echo "error: no dx-built 'beamer' binary with a sibling assets/ directory found under target/." >&2
    echo "Run 'dx build --release' first — a plain 'cargo build' does not produce assets." >&2
    exit 1
fi

echo "Found binary: $exe"
echo "Found assets: $assets_src"

# --- Install beamer ----------------------------------------------------------

mkdir -p "$bin_dir" "$lib_dir"
install -m755 "$exe" "$bin_dir/beamer"

# rm + recopy, not an rsync/copy-over: manganis content-hashes asset
# filenames, so a stale file from an older build would otherwise never get
# cleaned up.
rm -rf "$lib_dir/assets"
mkdir -p "$lib_dir/assets"
cp -r "$assets_src/." "$lib_dir/assets/"

echo "Installed beamer to $bin_dir/beamer"
echo "Installed assets to $lib_dir/assets/"

# --- Install GNOME extension sources -----------------------------------------
#
# Settings → Injection installs the helper from these files:
# `install::gnome_extension::locate_source_dir` probes
# <exe>/../share/beamer/extension/<uuid>, i.e. ~/.local/share/... for this
# layout. Without them the Install button fails outside a checkout, where
# the ./extension fallback no longer resolves.

ext_uuid="beamer-focus@beamer.app"
ext_src="extension/$ext_uuid"
ext_dst="$HOME/.local/share/beamer/extension/$ext_uuid"
if [ -f "$ext_src/metadata.json" ]; then
    rm -rf "$ext_dst"
    mkdir -p "$ext_dst"
    cp -r "$ext_src/." "$ext_dst/"
    echo "Installed extension sources to $ext_dst/"
else
    echo "warning: $ext_src/metadata.json not found; skipping extension sources." >&2
fi

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
