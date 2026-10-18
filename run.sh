#!/usr/bin/env bash
set -euo pipefail
studio_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
    if [[ -d "$studio_dir/../../work/studio-target" ]]; then
        export CARGO_TARGET_DIR="$studio_dir/../../work/studio-target"
    else
        export CARGO_TARGET_DIR="$studio_dir/target"
    fi
fi
export WAIFU_STUDIO_BUNDLE="${WAIFU_STUDIO_BUNDLE:-$studio_dir/assets/characters/bundle.json}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
exec cargo run --locked --manifest-path "$studio_dir/Cargo.toml" --bin waifu-studio -- "$@"
