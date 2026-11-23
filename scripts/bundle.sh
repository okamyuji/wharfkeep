#!/bin/sh
# リリースビルドを target/wharfkeep.app に組む。
# Cargo.toml の版は「次に組む .app の版」。組めたら patch を1つ上げ、次のビルドに備える。
set -eu
cd "$(dirname "$0")/.."
# [package] の版は行頭の version。依存の version はインラインの表の中なので当たらない。
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || {
  echo "bundle: Cargo.toml の版 '$version' が X.Y.Z の形ではありません" >&2
  exit 1
}
cargo build --release --locked
app=target/wharfkeep.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp assets/Info.plist "$app/Contents/Info.plist"
for key in CFBundleShortVersionString CFBundleVersion; do
  /usr/libexec/PlistBuddy -c "Delete :$key" "$app/Contents/Info.plist" >/dev/null 2>&1 || true
  /usr/libexec/PlistBuddy -c "Add :$key string $version" "$app/Contents/Info.plist"
done
cp target/release/wharfkeep "$app/Contents/MacOS/wharfkeep"
plutil -lint "$app/Contents/Info.plist" >/dev/null

# release の仕事は組んだ版そのものを公開するので、版を上げない。
if [ "${WHARFKEEP_NO_BUMP:-0}" = 1 ]; then
  echo "$app ($version)"
  exit 0
fi
next=$(echo "$version" | awk -F. '{ printf "%d.%d.%d", $1, $2, $3 + 1 }')
sed -i '' "1,/^version = /s/^version = \"$version\"$/version = \"$next\"/" Cargo.toml
# CIは --locked で試験するので、Cargo.lock の wharfkeep の版も合わせる。
cargo update --workspace --offline --quiet
echo "$app ($version, next $next)"
