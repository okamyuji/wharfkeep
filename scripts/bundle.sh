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
# 印のない版を名乗るのは CI の release だけにする。手元の .app には組んだコミットを
# semver の build metadata で付け、同じ番号の release と見分けられるようにする。
# CFBundleVersion は macOS が版の比較に使うので数字のままにする。
if [ "${WHARFKEEP_RELEASE:-0}" = 1 ]; then
  shown=$version
else
  rev=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
  git diff --quiet HEAD 2>/dev/null || rev="$rev.dirty"
  shown="$version+local.$rev"
fi
for key in CFBundleShortVersionString CFBundleVersion; do
  /usr/libexec/PlistBuddy -c "Delete :$key" "$app/Contents/Info.plist" >/dev/null 2>&1 || true
done
/usr/libexec/PlistBuddy -c "Add :CFBundleShortVersionString string $shown" "$app/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Add :CFBundleVersion string $version" "$app/Contents/Info.plist"
cp target/release/wharfkeep "$app/Contents/MacOS/wharfkeep"
plutil -lint "$app/Contents/Info.plist" >/dev/null

# release は組んだ版そのものを公開し、試験のための組み立ては版を進めない。
if [ "${WHARFKEEP_RELEASE:-0}" = 1 ] || [ "${WHARFKEEP_NO_BUMP:-0}" = 1 ]; then
  echo "$app ($shown)"
  exit 0
fi
next=$(echo "$version" | awk -F. '{ printf "%d.%d.%d", $1, $2, $3 + 1 }')
sed -i '' "1,/^version = /s/^version = \"$version\"$/version = \"$next\"/" Cargo.toml
# CIは --locked で試験するので、Cargo.lock の wharfkeep の版も合わせる。
cargo update --workspace --offline --quiet
echo "$app ($shown, next $next)"
