#!/bin/sh
# リリースビルドを target/wharfkeep.app に組む。
set -eu
cd "$(dirname "$0")/.."
cargo build --release --locked
app=target/wharfkeep.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp assets/Info.plist "$app/Contents/Info.plist"
cp target/release/wharfkeep "$app/Contents/MacOS/wharfkeep"
plutil -lint "$app/Contents/Info.plist" >/dev/null
echo "$app"
