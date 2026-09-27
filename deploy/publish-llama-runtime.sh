#!/usr/bin/env bash
# Packages the K2-Horizon llama-server runtime (Windows on ARM64) as the zip
# that `src/components/`'s catalog downloads, and optionally publishes it.
#
# Usage, from a checkout with vendor/llama-k2horizon/ populated:
#   deploy/publish-llama-runtime.sh <N>            # build dist/<asset>, print the catalog entry
#   deploy/publish-llama-runtime.sh <N> --upload   # upload that exact file as release llama-runtime-k2h-<N>
#
# Two steps on purpose. The catalog pins the zip's sha256, so the file that
# gets uploaded must be byte-for-byte the one that was hashed. `--upload`
# never rebuilds; it ships whatever the build step left in dist/.
#
# The zip is built deterministically (sorted entries, fixed timestamps and
# permissions), so rebuilding from the same vendor files on the same machine
# reproduces the same sha256. A different zlib may compress differently,
# which is one more reason `--upload` doesn't rebuild.
#
# The release is created as a PRERELEASE and never marked latest. The app's
# self-updater asks GitHub for /releases/latest, which excludes prereleases.
# If this were ever "latest", every install would try to treat the runtime zip
# as an app update.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

n="${1:-}"
if ! [[ "$n" =~ ^[0-9]+$ ]]; then
    echo "usage: $0 <N> [--upload]   (N = runtime revision, e.g. 1)" >&2
    exit 2
fi
mode="${2:-build}"

tag="llama-runtime-k2h-$n"
asset="llama-runtime-k2h-$n-aarch64-pc-windows-msvc.zip"
out="dist/$asset"
url="https://github.com/MalakingOso/Beamer/releases/download/$tag/$asset"

if [ "$mode" = "--upload" ]; then
    if [ ! -f "$out" ]; then
        echo "error: $out not found; run '$0 $n' first and paste its entry into the catalog." >&2
        exit 1
    fi
    sha="$(sha256sum "$out" | cut -d' ' -f1)"
    echo "Uploading $out (sha256 $sha) as prerelease $tag"
    gh release create "$tag" "$out" \
        --prerelease --latest=false \
        --title "llama-server runtime (K2-Horizon) r$n" \
        --notes "Runtime component for Beamer's local extraction on Windows ARM64. Not an app release; downloaded by the app itself."
    exit 0
fi

src="vendor/llama-k2horizon"
# The 9 runtime files: the server exe, its 4 direct DLL deps, 3 ggml DLLs,
# and the release OpenMP runtime (only the debug variant ships in System32).
# The ini and launcher scripts are NOT in the zip: they're embedded in the
# exe from deploy/ and installer/k2horizon/ (see src/components/mod.rs).
files=(
    ggml-base.dll
    ggml-cpu.dll
    ggml.dll
    libomp140.aarch64.dll
    llama-common.dll
    llama-server-impl.dll
    llama-server.exe
    llama.dll
    mtmd.dll
)
for f in "${files[@]}"; do
    if [ ! -f "$src/$f" ]; then
        echo "error: $src/$f missing; populate $src/ from the fork build first." >&2
        exit 1
    fi
done

mkdir -p dist
python3 - "$src" "$out" "${files[@]}" <<'PY'
import sys, zipfile
src, out, names = sys.argv[1], sys.argv[2], sorted(sys.argv[3:])
with zipfile.ZipFile(out, "w") as z:
    for name in names:
        info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = 0o644 << 16
        info.create_system = 0
        with open(f"{src}/{name}", "rb") as f:
            z.writestr(info, f.read(), compresslevel=9)
PY

sha="$(sha256sum "$out" | cut -d' ' -f1)"
size="$(stat -c %s "$out")"

echo "Built $out"
echo
echo "Catalog entry for src/components/mod.rs (RUNTIME):"
echo "    version: \"k2h-$n\","
echo "    kind: Kind::Archive {"
echo "        url: \"$url\","
echo "        sha256: \"$sha\","
echo "        size: $size,"
echo "    },"
echo
echo "Then publish it before shipping a build that carries this entry:"
echo "    $0 $n --upload"
