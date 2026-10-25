#!/usr/bin/env bash
set -euo pipefail
studio_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [[ -x "${CARGO_HOME:-$HOME/.cargo}/bin/cargo" ]]; then
    export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
fi
cd -- "$studio_dir"
if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
    export CARGO_TARGET_DIR="$studio_dir/target/linux"
fi
export WAIFU_STUDIO_ASSET_ROOT="${WAIFU_STUDIO_ASSET_ROOT:-$studio_dir/assets}"
export WAIFU_STUDIO_BUNDLE="${WAIFU_STUDIO_BUNDLE:-$WAIFU_STUDIO_ASSET_ROOT/characters/bundle.json}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-1}"
exec cargo run --release --locked --manifest-path "$studio_dir/Cargo.toml" --bin waifu-studio -- "$@"
