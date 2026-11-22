#!/bin/bash
# target/wharfkeep.app をOSから操作して確かめる。メニューバーからGUIを開いて閉じる流れ、状態ファイル、
# メニューからのインスタンスの停止と起動、ログイン時の起動（launchd）を試す。
# 先に scripts/bundle.sh を実行しておく。アクセシビリティと画面収録の許可が要る。
# colimaのインスタンス default を一度止めて起動し直す。終わると、自動起動を有効にしたまま
# launchdからデーモンを動かした状態にし、default を始めの状態に戻す。
#
# macOSは前面のアプリがある画面のメニューバーだけをAXに出し、ノッチなどで隠れた項目は出さない。
# メニューバーを引く直前に、外部画面にあるアプリ（WHARFKEEP_E2E_FRONT_APP、既定はZed）を前面にする。
set -euo pipefail
cd "$(dirname "$0")/.."
root=$(pwd -P)
app="$root/target/wharfkeep.app"
exe="$app/Contents/MacOS/wharfkeep"
out="$root/target/e2e"
data="$HOME/Library/Application Support/work.okamyuji.wharfkeep"
state="$data/state.json"
plist="$HOME/Library/LaunchAgents/wharfkeep.plist"
domain="gui/$(id -u)"
service="$domain/wharfkeep"
front_app=${WHARFKEEP_E2E_FRONT_APP:-Zed}
mkdir -p "$out"

fail() {
  echo "e2e: 失敗: $*" >&2
  exit 1
}

# 自分のコマンド行（パターンを含む）に当たらないよう、実行ファイルのフルパスで完全一致させる。
# パスの「.」などを正規表現の記号として読ませない。
exe_re=$(printf '%s' "$exe" | sed 's/[][\\.*^$+?(){}|]/\\&/g')
daemon_pids() { pgrep -fx "$exe_re( daemon)?" || true; }
gui_pids() { pgrep -fx "$exe_re gui" || true; }
count() { "$@" | grep -c . || true; }
# wait_untilに渡す条件。引数で$(...)を渡すと一度だけ展開されて値が固まるので、関数で数え直す。
daemons_are() { [ "$(count daemon_pids)" -eq "$1" ]; }
guis_are() { [ "$(count gui_pids)" -eq "$1" ]; }

# awkを途中で抜けるとlaunchctlがSIGPIPEで終わり、pipefailで失敗になるので最後まで読む。
launchd_pid() { launchctl print "$service" 2>/dev/null | awk '$1 == "pid" && $2 == "=" && !n++ { print $3 }' || true; }
launchd_runs() { [ -n "$(launchd_pid)" ] && daemons_are 1; }
launchd_restarted() { local p; p=$(launchd_pid); [ -n "$p" ] && [ "$p" != "$1" ] && daemons_are 1; }

colima_running() { colima list --json 2>/dev/null | jq -es 'any(.[]; .name == "default" and .status == "Running")' >/dev/null; }
colima_was_running=0
colima_running && colima_was_running=1

# 試験の後は利用者の環境に合わせ、自動起動を有効のままlaunchdのデーモンを動かしておく。
restore() {
  pkill -fx "$exe_re gui" || true
  local lp p
  lp=$(launchd_pid)
  for p in $(daemon_pids); do
    [ "$p" = "$lp" ] || kill "$p" 2>/dev/null || true
  done
  if [ -f "$plist" ]; then
    launchctl print "$service" >/dev/null 2>&1 || launchctl bootstrap "$domain" "$plist" || true
    [ -n "$(launchd_pid)" ] || launchctl kickstart "$service" || true
  fi
  if [ "$colima_was_running" = 1 ] && ! colima_running; then
    colima start default >/dev/null 2>&1 || true
  fi
}
on_exit() {
  local rc=$?
  restore
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
# ほかのアプリが前面を奪うことがあるので、前面にしてから同じスクリプトの中でメニューバーを引く。
# 開いているメニューは前面を替えると閉じるので、メニューの中を引くときはaxを使う。
bar() {
  timeout 10 osascript -e "tell application \"$front_app\" to activate" -e "delay 0.3" \
    -e "tell application \"System Events\" to tell (first process whose unix id is $1)" -e "$2" -e "end tell"
}

shot() { screencapture -x "$out/$1.png"; }
shot_rect() { screencapture -x -R "$2" "$out/$1.png"; }

item_geom() {
  bar "$1" 'tell menu bar item 1 of menu bar 2
if subrole is not "AXMenuExtra" then error "not a status item"
set {x, y} to position
set {w, h} to size
end tell
return (x as text) & " " & (y as text) & " " & (w as text) & " " & (h as text)' 2>/dev/null
}
status_item_visible() { item_geom "$1" >/dev/null; }
# VoiceOverが読むアイコンの説明。色が見えなくても状態が分かる。
icon_desc() { bar "$1" 'get description of menu bar item 1 of menu bar 2' 2>/dev/null; }
icon_is() { [ "$(icon_desc "$1")" = "$2" ]; }

# メニューを閉じていてもAXはメニューの項目を返すので、メニューが項目の真下にあることで開いたと見なす。
menu_open_below() {
  local pid=$1 ix=$2 iy=$3 ih=$4 pos mx my
  pos=$(ax "$pid" 'get position of menu 1 of menu bar item 1 of menu bar 2' 2>/dev/null | tr -d ' ') || return 1
  mx=${pos%,*} my=${pos#*,}
  ((my >= iy + ih && my <= iy + ih + 20 && mx > ix - 300 && mx < ix + 300))
}

# ステータス項目をAXPressで押してメニューを開き、項目を選ぶ。VoiceOverがメニューを開くのと同じ経路。
# $2は項目の名前。「last」なら最後の項目（終了）を選ぶ。言語によらず選べる。
choose_menu_item() {
  local pid=$1 item=$2 stage=$3 ix iy iw ih
  read -r ix iy iw ih <<<"$(item_geom "$pid")" || true
  [ -n "${ih:-}" ] || fail "メニューバー項目がAXに見えません。${front_app}を外部画面に置いてください（内蔵画面ではノッチの裏に隠れる）"
  local try
  for try in 1 2 3; do
    # 開いたメニューの中で操作が終わるまでAXPressが戻らないことがあるので、待たずに先へ進む。
    bar "$pid" 'click menu bar item 1 of menu bar 2' >/dev/null 2>&1 &
    wait_until 5 menu_open_below "$pid" "$ix" "$iy" "$ih" && break
    [ "$try" = 3 ] && fail "メニューが開きません"
  done
  shot_rect "$stage" "$((ix + iw / 2 - 300)),$iy,600,220"
  if [ "$item" = last ]; then
    ax "$pid" "click last menu item of menu 1 of menu bar item 1 of menu bar 2" >/dev/null
  else
    ax "$pid" "click menu item \"$item\" of menu 1 of menu bar item 1 of menu bar 2" >/dev/null
  fi
  wait || true
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

# 状態ファイルの default。例: state_default_is Running Ready
state_default_is() {
  jq -e --arg s "$1" --arg d "$2" \
    'any(.instances[]; .name == "default" and .status == $s) and (.docker.default | has($d))' \
    "$state" >/dev/null 2>&1
}
state_has_default() { jq -e 'any(.instances[]; .name == "default")' "$state" >/dev/null 2>&1; }
show_state() { jq -c '{instances: [.instances[] | {name, status}], docker: (.docker | map_values(keys[0]))}' "$state"; }

# 前回の試験が残したlaunchdのデーモンも止める。launchdに残すと止めても起こし直される。
launchctl bootout "$service" 2>/dev/null || true
pkill -fx "$exe_re( daemon| gui)?" || true
wait_until 10 daemons_are 0 || fail "前回のデーモンが残っています"
wait_until 10 guis_are 0 || fail "前回のGUIが残っています"
[ -x "$exe" ] || fail "$exe がありません。先に scripts/bundle.sh を実行してください"
[ "$colima_was_running" = 1 ] || fail "colimaのインスタンス default を起動してから実行してください"
# 前回の状態ファイルで5秒の確かめが通ってしまわないよう消す。設定は既定（自動起動は有効）から始める。
rm -f "$state" "$data/settings.json"

# AXの名前を固定するため、英語で通しの流れを試す。
echo "== en"
# (1)(2) .appを開き、デーモンが1つだけ立つまで待つ。
open --env WHARFKEEP_LANG=en "$app"
started=$SECONDS
wait_until 10 daemons_are 1 || fail "デーモンが1つになりません: $(daemon_pids | tr '\n' ' ')"
dpid=$(daemon_pids)
echo "(1)(2) デーモン pid=$dpid"

# 状態ファイルは5秒以内に書かれ、default が入る。
wait_until 5 state_has_default || fail "5秒以内に状態ファイルへ default が書かれません"
echo "(state) $((SECONDS - started))秒以内に書かれた: $(show_state)"

# 自動起動 (1) 初回の起動でlaunchdに登録される。
wait_until 10 launchctl print "$service" >/dev/null 2>&1 || fail "launchctl print $service が失敗します"
grep -qF '<string>daemon</string>' "$plist" || fail "plistに daemon の引数がありません"
grep -qF '<key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>' "$plist" || fail "plistにKeepAliveがありません"
echo "(autostart-1) launchctl print $service が成功し、plistに daemon と KeepAlive がある"

wait_until 10 status_item_visible "$dpid" || fail "メニューバー項目がAXに見えません。${front_app}を外部画面に置いてください（内蔵画面ではノッチの裏に隠れる）"
shot "en-1-daemon"

# (3) メニューバー項目を押し、「Open Window」を選ぶ。
choose_menu_item "$dpid" "Open Window" "en-3-menu"
echo "(3) AXPressで開いたメニューの「Open Window」を選んだ"

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

# (7) メニューから default を止めて起動し直す。アイコンの説明と状態ファイルが追いつくことを確かめる。
before=$(icon_desc "$dpid")
read -r ix iy iw ih <<<"$(item_geom "$dpid")"
icon_rect="$((ix - 40)),$iy,$((iw + 80)),$ih"
shot_rect "en-7a-icon-before" "$icon_rect"
choose_menu_item "$dpid" "Stop default" "en-7b-menu-stop"
wait_until 120 state_default_is Stopped Unavailable || fail "default が止まったことが状態ファイルに出ません: $(show_state)"
wait_until 10 icon_is "$dpid" "wharfkeep: no instance running" || fail "アイコンが実行中なしになりません: $(icon_desc "$dpid")"
shot_rect "en-7c-icon-stopped" "$icon_rect"
echo "(7) 止めた: $(show_state) / アイコン「$(icon_desc "$dpid")」（前は「$before」）"
choose_menu_item "$dpid" "Start default" "en-7d-menu-start"
wait_until 240 state_default_is Running Ready || fail "default が起動したことが状態ファイルに出ません: $(show_state)"
wait_until 10 icon_is "$dpid" "$before" || fail "アイコンが「$before」に戻りません: $(icon_desc "$dpid")"
shot_rect "en-7e-icon-started" "$icon_rect"
echo "(7) 起動した: $(show_state) / アイコン「$(icon_desc "$dpid")」"

# (8) メニューの「Quit」で、launchdの外で起動したデーモンを止める。
choose_menu_item "$dpid" "Quit" "en-8-menu-quit"
wait_until 10 daemons_are 0 || fail "「Quit」でデーモンが終わりません"
echo "(8) 「Quit」でデーモンが終わった"

echo "== autostart"
# (2) ログイン時と同じく、launchdに読み込ませると起動する。
launchctl bootout "$service" 2>/dev/null || true
launchctl bootstrap "$domain" "$plist"
wait_until 10 launchd_runs || fail "bootstrapでデーモンが起動しません"
lpid=$(launchd_pid)
echo "(autostart-2) bootstrapでデーモン pid=$lpid が起動した"

# (3) 異常終了すると起動し直す。launchdは10秒より短い間隔では起こし直さない。
kill -9 "$lpid"
wait_until 20 launchd_restarted "$lpid" || fail "kill -9 の後に起動し直しません"
lpid2=$(launchd_pid)
echo "(autostart-3) kill -9 の後、pid=$lpid2 で起動し直した"

# (4) メニューの終了で止めると起動し直さない。launchdのデーモンは言語を選ばないので最後の項目で選ぶ。
wait_until 10 status_item_visible "$lpid2" || fail "launchdのデーモンのメニューバー項目がAXに見えません"
choose_menu_item "$lpid2" last "autostart-4-menu-quit"
wait_until 10 daemons_are 0 || fail "終了でデーモンが終わりません"
if wait_until 15 daemons_are 1; then
  fail "終了で止めたのに起動し直しました"
fi
printed=$(launchctl print "$service")
grep -q 'last exit code = 0' <<<"$printed" || fail "終了コードが0ではありません"
echo "(autostart-4) 終了の後、15秒待っても起動し直さない（last exit code = 0）"

# 日本語の文言が描けることを確かめる。メニューバーの流れは上で確かめたのでGUIだけを開く。
echo "== ja"
WHARFKEEP_LANG=ja "$exe" gui >/dev/null 2>&1 &
wait_until 10 guis_are 1 || fail "GUIのプロセスが立ちません（ja）"
gpid=$(gui_pids)
check_reload_button "$gpid" "再読み込み" "再読み込みした回数:"
shot_rect "ja-5-gui" "$(window_rect "$gpid")"
close_window "$gpid"
echo "(ja) 「再読み込み」を押し「再読み込みした回数: 1」になり、閉じるとGUIが終わった"

launchctl kickstart "$service"
wait_until 10 launchd_runs || fail "launchdからデーモンを起動し直せません"
echo "(restore) launchdのデーモン pid=$(launchd_pid) を動かしたまま終える"

echo "e2e: 成功（画面は ${out}）"
