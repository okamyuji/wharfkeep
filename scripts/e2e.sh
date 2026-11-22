#!/bin/bash
# target/wharfkeep.app をOSから操作し、メニューバーからGUIを開き、閉じるとGUIだけが終わることを確かめる。
# 先に scripts/bundle.sh を実行しておく。アクセシビリティと画面収録の許可が要る。
#
# macOSは前面のアプリがある画面のメニューバーだけをAXに出し、ノッチなどで隠れた項目は出さない。
# 項目を探す前にFinderを前面にする。ただしFinderを前面にしても使われる画面は変わらないので、
# 直前に前面だったアプリの画面で項目が隠れていれば、AXには出ない。
set -euo pipefail
cd "$(dirname "$0")/.."
root=$(pwd -P)
app="$root/target/wharfkeep.app"
exe="$app/Contents/MacOS/wharfkeep"
out="$root/target/e2e"
mkdir -p "$out"

fail() {
  echo "e2e: 失敗: $*" >&2
  exit 1
}

# 自分のコマンド行（パターンを含む）に当たらないよう、実行ファイルのフルパスで完全一致させる。
daemon_pids() { pgrep -fx "$exe( daemon)?" || true; }
gui_pids() { pgrep -fx "$exe gui" || true; }
count() { "$@" | grep -c . || true; }
# wait_untilに渡す条件。引数で$(...)を渡すと一度だけ展開されて値が固まるので、関数で数え直す。
daemons_are() { [ "$(count daemon_pids)" -eq "$1" ]; }
guis_are() { [ "$(count gui_pids)" -eq "$1" ]; }

cleanup() { pkill -f "wharfkeep.app/Contents/MacOS/wharfkeep" || true; }
on_exit() {
  local rc=$?
  cleanup
  exit "$rc"
}
trap on_exit EXIT

# 条件が成り立つまで繰り返す。最大 $1 秒。
wait_until() {
  local limit=$1
  shift
  local end=$((SECONDS + limit))
  until "$@"; do
    ((SECONDS < end)) || return 1
    sleep 0.2
  done
}

# デーモンとGUIは同じプロセス名なので、System Eventsが返す参照（名前で引く）を使い回さず、
# pidで引いた1つの式で問い合わせる。
ax() { timeout 10 osascript -e "tell application \"System Events\" to tell (first process whose unix id is $1)" -e "$2" -e "end tell"; }

shot() { screencapture -x "$out/$1.png"; }
shot_rect() { screencapture -x -R "$2" "$out/$1.png"; }

item_geom() {
  ax "$1" 'tell menu bar item 1 of menu bar 2
if subrole is not "AXMenuExtra" then error "not a status item"
set {x, y} to position
set {w, h} to size
end tell
return (x as text) & " " & (y as text) & " " & (w as text) & " " & (h as text)' 2>/dev/null
}
status_item_visible() { item_geom "$1" >/dev/null; }

# メニューを閉じていてもAXはメニューの項目を返すので、メニューが項目の真下にあることで開いたと見なす。
menu_open_below() {
  local pid=$1 ix=$2 iy=$3 ih=$4 pos mx my
  pos=$(ax "$pid" 'get position of menu 1 of menu bar item 1 of menu bar 2' 2>/dev/null | tr -d ' ') || return 1
  mx=${pos%,*} my=${pos#*,}
  ((my >= iy + ih && my <= iy + ih + 20 && mx > ix - 300 && mx < ix + 300))
}

mouse_click() {
  timeout 10 osascript -l JavaScript -e "ObjC.import('CoreGraphics');
const p = \$.CGPointMake($1, $2);
for (const t of [\$.kCGEventLeftMouseDown, \$.kCGEventLeftMouseUp]) {
  \$.CGEventPost(\$.kCGHIDEventTap, \$.CGEventCreateMouseEvent(null, t, p, \$.kCGMouseButtonLeft));
}" >/dev/null
}

# ステータス項目へ実際のマウスクリックを送ってメニューを開き、項目を選ぶ。
# tray-iconは自前のmouseDown処理の中でだけメニューを付けるため、AXPressではメニューが開かない。
choose_menu_item() {
  local pid=$1 item=$2 stage=$3 ix iy iw ih cx cy
  read -r ix iy iw ih <<<"$(item_geom "$pid")" || true
  [ -n "${ih:-}" ] || fail "メニューバー項目がAXに見えません。外部画面のアプリを前面にしてから実行してください（内蔵画面ではノッチの裏に隠れることがある）"
  cx=$((ix + iw / 2)) cy=$((iy + ih / 2))
  # 開いているメニューを押すと閉じるので、押し直すのは閉じたままのときだけにする。
  local try
  for try in 1 2; do
    mouse_click "$cx" "$cy"
    wait_until 5 menu_open_below "$pid" "$ix" "$iy" "$ih" && break
    [ "$try" = 2 ] && fail "メニューが開きません"
  done
  shot_rect "$stage" "$((cx - 300)),$iy,600,200"
  ax "$pid" "click menu item \"$item\" of menu 1 of menu bar item 1 of menu bar 2" >/dev/null
}

contents() { ax "$1" 'get entire contents of window "wharfkeep"' 2>/dev/null | tr ',' '\n' || true; }
# 例: has_element <pid> "button Reload"
has_element() { contents "$1" | grep -qF " $2 of "; }

window_rect() {
  ax "$1" 'tell window "wharfkeep"
set {x, y} to position
set {w, h} to size
end tell
return (x as text) & "," & (y as text) & "," & (w as text) & "," & (h as text)'
}

# ボタンを押し、押した回数の表示が変わることで、eguiが押されたことを受け取ったと確かめる。
# AccessKitは最初の問い合わせの後に木を作るので、見えるまで繰り返す。
check_reload_button() {
  local gpid=$1 reload=$2 count_label=$3
  wait_until 15 has_element "$gpid" "button $reload" || fail "ボタン「${reload}」がAXで見えません"
  has_element "$gpid" "static text $count_label 0" || fail "押す前の表示「${count_label} 0」がありません"
  # egui（AccessKit）の木は「window > group > UI element > button」になる。
  ax "$gpid" "click button \"$reload\" of UI element 1 of group 1 of window \"wharfkeep\"" >/dev/null
  wait_until 5 has_element "$gpid" "static text $count_label 1" || fail "押した後の表示「${count_label} 1」になりません"
}

close_window() {
  ax "$1" 'click (first button of window "wharfkeep" whose subrole is "AXCloseButton")' >/dev/null
  wait_until 10 guis_are 0 || fail "ウィンドウを閉じてもGUIのプロセスが残ります"
}

cleanup
wait_until 10 daemons_are 0 || fail "前回のデーモンが残っています"
wait_until 10 guis_are 0 || fail "前回のGUIが残っています"
[ -x "$exe" ] || fail "$exe がありません。先に scripts/bundle.sh を実行してください"

# AXの名前を固定するため、英語で通しの流れを試す。
echo "== en"
# (1)(2) .appを開き、デーモンが1つだけ立つまで待つ。
open --env WHARFKEEP_LANG=en "$app"
wait_until 10 daemons_are 1 || fail "デーモンが1つになりません: $(daemon_pids | tr '\n' ' ')"
dpid=$(daemon_pids)
timeout 10 osascript -e 'tell application "Finder" to activate' >/dev/null
wait_until 10 status_item_visible "$dpid" || fail "メニューバー項目がAXに見えません。外部画面のアプリを前面にしてから実行してください（内蔵画面ではノッチの裏に隠れることがある）"
shot "en-1-daemon"
echo "(1)(2) デーモン pid=$dpid"

# (3) メニューバー項目を押し、「Open Window」を選ぶ。
choose_menu_item "$dpid" "Open Window" "en-3-menu"
echo "(3) メニューの「Open Window」を選んだ"

# (4) GUIのプロセスが立つ。
wait_until 10 guis_are 1 || fail "GUIのプロセスが立ちません"
gpid=$(gui_pids)
echo "(4) GUI pid=$gpid"

# (5) ボタンを押せる。
check_reload_button "$gpid" "Reload" "Reloads:"
shot_rect "en-5-gui" "$(window_rect "$gpid")"
echo "(5) 「Reload」を押し「Reloads: 1」になった"

# (6) ウィンドウを閉じると、GUIは終わりデーモンは残る。
close_window "$gpid"
[ "$(daemon_pids)" = "$dpid" ] || fail "デーモンが残っていません"
shot "en-6-closed"
echo "(6) GUIは終了し、デーモン pid=$dpid は残った"

pkill -fx "$exe" || true
wait_until 10 daemons_are 0 || fail "デーモンが終わりません"

# 日本語の文言が描けることを確かめる。メニューバーの流れは上で確かめたのでGUIだけを開く。
echo "== ja"
WHARFKEEP_LANG=ja "$exe" gui >/dev/null 2>&1 &
wait_until 10 guis_are 1 || fail "GUIのプロセスが立ちません（ja）"
gpid=$(gui_pids)
check_reload_button "$gpid" "再読み込み" "再読み込みした回数:"
shot_rect "ja-5-gui" "$(window_rect "$gpid")"
close_window "$gpid"
echo "(ja) 「再読み込み」を押し「再読み込みした回数: 1」になり、閉じるとGUIが終わった"

echo "e2e: 成功（画面は ${out}）"
