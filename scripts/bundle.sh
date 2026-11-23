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
# 印のない版を名乗るのは CI の release だけにする。手元の .app には組んだコミットを
# semver の build metadata で付け、同じ番号の release と見分けられるようにする。
# Apple は CFBundleShortVersionString を数字とピリオドだけと定めるが、手元の組み立ては
# 公開しないので見分けやすさを優先する。CFBundleVersion は macOS が版の比較に使うので数字のままにする。
if [ "${WHARFKEEP_RELEASE:-0}" = 1 ]; then
  [ -n "${GITHUB_ACTIONS:-}" ] || {
    echo "bundle: WHARFKEEP_RELEASE=1 は CI の release だけが使います" >&2
    exit 1
  }
  shown=$version
else
  # 外から渡された GIT_DIR や親のリポジトリを読まず、このディレクトリ自身のリポジトリだけを見る。
  g() { env -u GIT_DIR -u GIT_WORK_TREE git "$@"; }
  # 文字列でなく同じディレクトリかで比べる。macOS は大文字小文字の違うパスでも同じ場所を指す。
  top=$(g rev-parse --show-toplevel 2>/dev/null || true)
  if [ -n "$top" ] && [ "$top" -ef . ]; then
    rev=$(g rev-parse --short HEAD 2>/dev/null || echo unknown)
    # 組んだあとの版上げだけで dirty にしない。未追跡のファイルは変更に数える。
    [ -z "$(g status --porcelain -- . ':!Cargo.toml' ':!Cargo.lock')" ] || rev="$rev.dirty"
  else
    rev=nogit
  fi
  shown="$version+local.$rev"
fi
cargo build --release --locked
app=target/wharfkeep.app
rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp assets/Info.plist "$app/Contents/Info.plist"
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
