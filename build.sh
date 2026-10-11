#!/usr/bin/env bash
set -euo pipefail
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
echo "Bundling TraceTuner in Release mode..."
cargo xtask bundle trace_tuner --release --features gui
test -e target/bundled/TraceTuner.vst3
test -e target/bundled/TraceTuner.clap
mkdir -p tmp
snapshot=$(mktemp -d tmp/release_"$(date +%Y%m%d_%H%M%S)"_XXXXXX)
cp -R target/bundled/TraceTuner.vst3 target/bundled/TraceTuner.clap "$snapshot/"
if [[ -d bin ]]; then
    cp -R bin "$snapshot/previous-bin"
fi
mkdir -p bin
for artifact in TraceTuner.vst3 TraceTuner.clap; do
    if [[ -d "$snapshot/$artifact" ]]; then
        mkdir -p "bin/$artifact"
        cp -R "$snapshot/$artifact/." "bin/$artifact/"
    else
        cp "$snapshot/$artifact" "bin/$artifact"
    fi
done
echo "Build complete: bin/ (snapshot: $snapshot)"
