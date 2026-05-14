#!/usr/bin/env bash
# TraceTuner Build Script (Bash)
# This script builds bundled plugin artifacts using NIH-plug's bundler.

set -e

echo "Bundling TraceTuner in Release mode..."
rm -rf "target/bundled/TraceTuner.vst3" "target/bundled/TraceTuner.clap"
cargo xtask bundle trace_tuner --release --features gui

mkdir -p bin
mkdir -p tmp

rm -rf "bin/TraceTuner.vst3" "bin/TraceTuner.clap"
cp -R "target/bundled/TraceTuner.vst3" "bin/TraceTuner.vst3"
cp -R "target/bundled/TraceTuner.clap" "bin/TraceTuner.clap"

echo "Build complete! Plugins are located in the bin/ directory."

echo "Creating timestamped release in tmp/..."
dt=$(date +%Y%m%d_%H%M%S)
dir="tmp/release_$dt"
mkdir -p "$dir"
cp -R "target/bundled/TraceTuner.vst3" "$dir/TraceTuner.vst3"
cp -R "target/bundled/TraceTuner.clap" "$dir/TraceTuner.clap"
echo "Snapshot saved to $dir"
