# Coldcard simulator build inputs

These files are consumed by the `coldcard_sim` CI job (and by anyone
following [AGENTS.md](../../../AGENTS.md)) to build the upstream
[`coldcard-mpy`](https://github.com/Coldcard/firmware) simulator that
`hwi-rs`'s integration tests drive.

| File                     | Origin                                       |
| ------------------------ | -------------------------------------------- |
| `Dockerfile`             | Local — pinned `ubuntu:24.04` build env.     |
| `ubuntu24_mpy.patch`     | Vendored from [bitcoin-core/HWI@main `test/data/coldcard-multisig.patch` peer file](https://github.com/bitcoin-core/HWI). Suppresses GCC dangling-pointer / enum-int-mismatch errors that are fatal under `-Werror`. |
| `bech32.patch`           | Vendored from `external/libngu/bech32.patch` in the firmware tree. Drops `static` from `convert_bits()` so the firmware can link against it. |
| `mpy.patch`              | Vendored from `external/libngu/mpy.patch` in the firmware tree. micropython glue fix. |
| `build.sh`               | Driver script: clones firmware at the pinned tag, applies the three patches, builds `mpy-cross` and the `coldcard-mpy` simulator inside the `cc-sim-builder` image. |
| `firmware-tag.txt`       | Pinned firmware tag the cache key is derived from. Bumping this invalidates the CI cache. |

Both `bech32.patch` and `mpy.patch` are also present at those paths
inside the firmware tree itself, but the build script cannot rely on
them being applied upstream.
