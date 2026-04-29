#!/usr/bin/env bash
# Build the Coldcard `coldcard-mpy` simulator from the upstream firmware
# tree at the tag pinned in `firmware-tag.txt`. Used by the
# `coldcard_sim` CI job and reproducible locally — see
# `AGENTS.md` for an interactive walk-through.
#
# Inputs (env):
#   WORK_DIR           Directory to materialise everything under.
#                      Default: $PWD/cc-sim-work
#   IMAGE              Podman/Docker image with build deps.
#                      Default: cc-sim-builder
#   CONTAINER_RUNTIME  podman | docker. Default: podman
#
# On success the simulator binary is at:
#   $WORK_DIR/firmware/unix/coldcard-mpy
# and `simulator.py` next to it is the Python entry point.

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$PWD/cc-sim-work}"
IMAGE="${IMAGE:-cc-sim-builder}"
CONTAINER_RUNTIME="${CONTAINER_RUNTIME:-podman}"
FIRMWARE_TAG="$(tr -d '[:space:]' < "$HERE/firmware-tag.txt")"

mkdir -p "$WORK_DIR"
cd "$WORK_DIR"

# 1. Build the build-environment image (idempotent, layer cache hits
#    when Dockerfile is unchanged).
"$CONTAINER_RUNTIME" build -t "$IMAGE" -f "$HERE/Dockerfile" "$HERE"

# 2. Clone (or reuse) the firmware tree at the pinned tag. We need a
#    full clone — submodules cannot be resolved with --depth 1.
if [[ ! -d firmware/.git ]]; then
    # The lwip submodule is hosted on git.savannah which stalls in CI;
    # rewrite to the GitHub mirror before cloning.
    git config --global --add url."https://github.com/lwip-tcpip/lwip.git".insteadOf "https://git.savannah.gnu.org/r/lwip.git" || true
    git config --global --add url."https://github.com/lwip-tcpip/lwip.git".insteadOf "https://git.savannah.nongnu.org/git/lwip.git" || true
    git clone --recursive https://github.com/Coldcard/firmware.git
fi

cd firmware
git fetch --tags origin
git checkout --force "$FIRMWARE_TAG"
git submodule update --init --recursive

# 3. Apply the three required patches. `git apply --check` lets us
#    skip cleanly when the cache already has them applied.
apply_once() {
    local repo="$1" patch="$2"
    if git -C "$repo" apply --check "$patch" 2>/dev/null; then
        git -C "$repo" apply "$patch"
        echo "applied $patch -> $repo"
    elif git -C "$repo" apply --reverse --check "$patch" 2>/dev/null; then
        echo "skipping $patch -> $repo (already applied)"
    else
        echo "patch $patch does not apply cleanly to $repo" >&2
        exit 1
    fi
}
apply_once external/micropython         "$HERE/ubuntu24_mpy.patch"
apply_once external/libngu/libs/bech32  "$HERE/bech32.patch"
apply_once external/libngu/libs/mpy     "$HERE/mpy.patch"

cd "$WORK_DIR"

# 4. Build mpy-cross + coldcard-mpy inside the pinned container.
"$CONTAINER_RUNTIME" run --rm \
    -v "$WORK_DIR:/work" \
    -w /work/firmware \
    "$IMAGE" \
    bash -lc '
        set -euo pipefail
        cd external/micropython/mpy-cross && make
        cd /work/firmware/unix
        ln -sf ../external/micropython/ports/unix/coldcard-mpy .
        make
    '

ls -l firmware/unix/coldcard-mpy
echo "OK: simulator built at $WORK_DIR/firmware/unix/coldcard-mpy"
