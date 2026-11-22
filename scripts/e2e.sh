#!/bin/bash
# target/wharfkeep.app をOSから操作して確かめる。メニューバーからGUIを開いて閉じる流れ、二度目の「Open Window」、
# デーモンを止めたときのGUIの表示、状態ファイル、
# コンテナ画面の行のボタン（停止と開始を1回ずつ押す）、メニューからのインスタンスの停止と起動、
# ログイン時の起動（launchd）を試す。
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

# 画面がロックされているとmacOSはウィンドウを描かず、AXからGUIの中身が見えない。
# grep -qが先に抜けるとioregがSIGPIPEで終わり、pipefailで偽になるので、出力を受けてから調べる。
screen_locked() { local s; s=$(ioreg -n Root -d1); [[ $s == *'"CGSSessionScreenIsLocked"=Yes'* ]]; }
if screen_locked; then
  fail "画面がロックされています。ロックを解除してから実行してください"
fi

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

# Dockerのソケットはcolimaに聞く（利用者のdocker contextにも、colimaの置き場所にも頼らない）。値は unix:// 付き。
socket_of() { colima status "$1" --json 2>/dev/null | jq -r '.docker_socket // empty'; }
# default が止まっていると colima status が失敗する。その場合は下の「起動してから実行してください」で止める。
default_sock=$(socket_of default || true)
iso_sock=

# 行のボタンの試験に使うコンテナ。colimaのdefaultのソケットを直接指す。
row_name=wk-e2e-row
# ソケットが取れないときは、docker contextの先（利用者の環境）へ流さずに失敗させる。
dk() {
  [ -n "$default_sock" ] || { echo "e2e: default のソケットが取れません" >&2; return 1; }
  DOCKER_HOST="$default_sock" docker "$@"
}
row_cleanup() { dk rm -f "${row_name}" >/dev/null 2>&1 || true; }
row_state() { jq -r --arg n "${row_name}" '.docker.default.Ready.containers[]? | select(.name == $n) | .state' "$state" 2>/dev/null; }
row_is() { [ "$(row_state)" = "$1" ]; }
row_ax_gone() { ! contents "$1" | grep -qF "${row_name}"; }
row_gone() { [ -z "$(row_state)" ]; }
row_up() {
  row_cleanup
  dk run -d --name "${row_name}" alpine:latest sleep 300 >/dev/null || fail "alpine:latest でコンテナ ${row_name} を作れません"
  wait_until 30 row_is running || fail "状態ファイルに実行中の ${row_name} が出ません"
}

# 削除と片付けの試験は、専用のインスタンスで行う。default の利用者のイメージ、ボリューム、
# ネットワーク、ビルドキャッシュを片付けで消さないため。途中で失敗しても restore が消す。
iso=wk-e2e
iso_created=0
dki() {
  [ -n "$iso_sock" ] || { echo "e2e: ${iso} のソケットが取れません" >&2; return 1; }
  DOCKER_HOST="$iso_sock" docker "$@"
}
iso_ready() { [ "$(jq -r --arg n "$iso" '.docker[$n] // {} | keys[0] // ""' "$state" 2>/dev/null)" = Ready ]; }
img_has() { dki image inspect "$1" >/dev/null 2>&1; }
img_gone() { ! img_has "$1"; }
vol_has() { dki volume inspect "$1" >/dev/null 2>&1; }
vol_gone() { ! vol_has "$1"; }
net_has() { dki network inspect "$1" >/dev/null 2>&1; }
net_gone() { ! net_has "$1"; }
no_volumes() { [ -z "$(dki volume ls -q)" ]; }
no_dangling() { [ -z "$(dki images -f dangling=true -q)" ]; }
cache_empty() { dki system df --format '{{.Type}}|{{.Size}}' | grep -qx 'Build Cache|0B'; }
# 例: elem_name <pid> checkbox "Remove the build cache"。名前の途中から、AXの完全な名前を取る。
# 場所の部分（「of UI element 3 of group 1 …」や確認ダイアログの「of window 1 of group 1 …」）を外してから名前を取る。
elem_name() {
  contents "$1" | sed -E 's/ of (UI element|window) [0-9]+ of group 1 of window .*$//' | sed -n "s/^ *$2 //p" | grep -F -- "$3" | head -1
}

# 試験の後は利用者の環境に合わせ、自動起動を有効のままlaunchdのデーモンを動かしておく。
restore() {
  pkill -fx "$exe_re gui" || true
  local lp p
  lp=$(launchd_pid)
  for p in $(daemon_pids); do
    [ "$p" = "$lp" ] || kill "$p" 2>/dev/null || true
  done
  # 自動起動をオフにした段階のあとで失敗すると、plistが消えたまま残る。設定を既定（オン）に戻し、
  # 起動したデーモンにplistを書かせてから止め、下でlaunchdに任せる。
  if [ ! -f "$plist" ]; then
    rm -f "$data/settings.json"
    "$exe" daemon >/dev/null 2>&1 &
    p=$!
    wait_until 10 test -f "$plist" || true
    kill "$p" 2>/dev/null || true
    wait_until 10 job_gone "$p" || true
  fi
  if [ -f "$plist" ]; then
    launchctl print "$service" >/dev/null 2>&1 || launchctl bootstrap "$domain" "$plist" || true
    [ -n "$(launchd_pid)" ] || launchctl kickstart "$service" || true
  fi
  if [ "$colima_was_running" = 1 ] && ! colima_running; then
    colima start default >/dev/null 2>&1 || true
  fi
  colima_running && row_cleanup
  if [ "$iso_created" = 1 ]; then
    colima delete "$iso" --force --data >/dev/null 2>&1 || true
  fi
}
# macOSの/bin/bash（3.2）は未定義の変数で止まったときもEXITのトラップに0を渡すので、最後まで来たかで判定する。
succeeded=0
on_exit() {
  local rc=$?
  # 見張りの中のsleepは別のプロセスなので、先に止める。残すと上限まで出力の管を開いたままにする。
  pkill -P "$watchdog" 2>/dev/null || true
  kill "$watchdog" 2>/dev/null || true
  restore
  if [ "$succeeded" != 1 ] && [ "$rc" = 0 ]; then
    rc=1
  fi
  exit "$rc"
}
trap on_exit EXIT
# 全体の上限。超えたら、そのとき実行していた行とコマンドを書いて失敗させる。
trap 'echo "e2e: 失敗: 上限 ${E2E_LIMIT:-1500} 秒を超えました（${LINENO}行目付近: ${BASH_COMMAND}）" >&2; exit 1' TERM
( sleep "${E2E_LIMIT:-1500}"; kill -TERM $$ ) &
watchdog=$!
# 実行中に画面が消えてロックされないようにする。-w で、このシェルが終われば一緒に終わる。
caffeinate -di -w $$ &

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
# 画像は目で見る記録で、合否はAXと状態ファイルで決める。画面の端などで撮れなくても失敗にしない。
shot_rect() { screencapture -x -R "$2" "$out/$1.png" || echo "e2e: 警告: $1 の画像を撮れません（範囲 $2）" >&2; }

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
job_gone() { ! kill -0 "$1" 2>/dev/null; }
# メニューが開いたあとで、前面のアプリが変わるなどして閉じることがある。閉じたら開き直して選び直す。
choose_menu_item() {
  local pid=$1 item=$2 stage=$3 ix iy iw ih
  read -r ix iy iw ih <<<"$(item_geom "$pid")" || true
  [ -n "${ih:-}" ] || fail "メニューバー項目がAXに見えません。${front_app}を外部画面に置いてください（内蔵画面ではノッチの裏に隠れる）"
  local target try click clicks=() chosen=0
  case "$item" in
    last) target="last menu item" ;;
    # 「ログイン時に起動」は終了の1つ前。言語によらず位置で選ぶ。
    login) target="menu item -2" ;;
    *) target="menu item \"$item\"" ;;
  esac
  for try in 1 2 3; do
    # 開いたメニューの中で操作が終わるまでAXPressが戻らないことがあるので、待たずに先へ進む。
    bar "$pid" 'click menu bar item 1 of menu bar 2' >/dev/null 2>&1 &
    clicks+=("$!")
    if wait_until 5 menu_open_below "$pid" "$ix" "$iy" "$ih"; then
      shot_rect "$stage" "$((ix + iw / 2 - 300)),$iy,600,220"
      if ax "$pid" "click $target of menu 1 of menu bar item 1 of menu bar 2" >/dev/null 2>&1; then
        chosen=1
        break
      fi
    fi
  done
  [ "$chosen" = 1 ] || fail "メニューを3回開いても「${item}」を押せません"
  # 自分が起こした押下だけを、上限つきで待つ。引数なしの wait は、試験が起こしたデーモンなど
  # ほかのバックグラウンドの子も待ち、終わらないと e2e が止まる。
  for click in "${clicks[@]}"; do
    wait_until 10 job_gone "$click" || { kill "$click" 2>/dev/null || true; }
    wait "$click" 2>/dev/null || true
  done
}

# eframeは隠れたウィンドウのUIを走らせないので、AXの木は隠れた時点のまま古くなる。
# 試験中に利用者が別のアプリを前面にしても読めるよう、GUIを前面に戻してから読む。
contents() {
  ax "$1" 'if not frontmost then set frontmost to true
get entire contents of window "wharfkeep"' 2>/dev/null | tr ',' '\n' || true
}
# 例: has_element <pid> "button Reload"
# 名前の途中までで探す。例: has_prefix <pid> "button Running: "
has_prefix() { contents "$1" | grep -qF " $2"; }
has_element() { contents "$1" | grep -qF " $2 of "; }

window_rect() {
  ax "$1" 'tell window "wharfkeep"
set {x, y} to position
set {w, h} to size
end tell
return (x as text) & "," & (y as text) & "," & (w as text) & "," & (h as text)'
}

# ウィンドウ内の要素を種類と名前で押す。通知や下のパネルの有無で添字が変わるので、group 1 の子を順に試す。
# 確認ダイアログ（egui::Modal）は子の「window 1」として最後に並ぶ。
# 例: ax_press <pid> checkbox Containers
ax_press() {
  ax "$1" "set done to false
repeat with i from 1 to (count UI elements of group 1 of window \"wharfkeep\")
try
click $2 \"$3\" of UI element i of group 1 of window \"wharfkeep\"
set done to true
exit repeat
end try
end repeat
if not done then error \"$2 $3 が見つかりません\""
}

# サイドバーの項目を押し、同じ名前の見出しが出ることで、eguiが押されたことを受け取ったと確かめる。
# AccessKitは最初の問い合わせの後に木を作るので、見えるまで繰り返す。
# サイドバーの項目はAXでは checkbox、見出しは UI element として出る。
check_navigation() {
  local gpid=$1 page=$2 other=$3
  wait_until 15 has_element "$gpid" "checkbox $page" || fail "サイドバーの「${page}」がAXで見えません"
  has_element "$gpid" "UI element $other" || fail "押す前の見出し「${other}」がありません"
  ax_press "$gpid" checkbox "$page" >/dev/null
  wait_until 5 has_element "$gpid" "UI element $page" || fail "押した後の見出し「${page}」が出ません"
}

front_pid() { timeout 10 osascript -e 'tell application "System Events" to get unix id of first process whose frontmost is true' 2>/dev/null; }
banner_gone() { ! contents "$1" | grep -qF "The daemon is not running"; }
front_is_not() { [ "$(front_pid)" != "$1" ]; }
is_front() { [ "$(front_pid)" = "$1" ]; }
window_count() { ax "$1" 'count windows'; }

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

# (5) サイドバーの項目を押せる。
check_navigation "$gpid" "Containers" "Dashboard"
shot_rect "en-5-gui" "$(window_rect "$gpid")"
echo "(5) 「Containers」を押し、見出しが「Containers」になった"

# (5r) コンテナ画面の行の色帯、状態の文字、1回押すだけのボタン。メニューも詳細画面もたどらない。
row_up
wait_until 15 has_element "$gpid" "button Stop ${row_name}" || fail "行の「Stop ${row_name}」がAXで見えません: $(contents "$gpid" | grep -i 'button' | sed 's/ of .*//' | tr '\n' '|')"
has_element "$gpid" "static text Running" || fail "行に状態の文字「Running」がありません"
shot_rect "en-5r-row-running" "$(window_rect "$gpid")"
ax_press "$gpid" button "Stop ${row_name}" >/dev/null || fail "「Stop ${row_name}」を押せません"
wait_until 60 row_is exited || fail "「Stop ${row_name}」を1回押しても ${row_name} が止まりません: $(row_state)"
wait_until 15 has_element "$gpid" "button Start ${row_name}" || fail "止まった行に「Start ${row_name}」が出ません"
has_element "$gpid" "static text Stopped" || fail "止まった行に状態の文字「Stopped」がありません"
shot_rect "en-5r-row-stopped" "$(window_rect "$gpid")"
echo "(5r) 「Stop ${row_name}」を1回押して ${row_name} が exited になった"
ax_press "$gpid" button "Start ${row_name}" >/dev/null || fail "「Start ${row_name}」を押せません"
wait_until 60 row_is running || fail "「Start ${row_name}」を1回押しても ${row_name} が動きません: $(row_state)"
wait_until 15 has_element "$gpid" "button Stop ${row_name}" || fail "動き直した行に「Stop ${row_name}」が出ません"
echo "(5r) 「Start ${row_name}」を1回押して ${row_name} が running に戻った"
row_cleanup
wait_until 30 row_gone || fail "${row_name} を消しても状態ファイルに残ります"
wait_until 15 row_ax_gone "$gpid" || fail "${row_name} を消しても行が残ります"
echo "(5r) ${row_name} を消し、行も消えた"

# (5d) ダッシュボードのインスタンスの枠とコンテナの枠に、内訳と左端の色帯が出る。
row_up
ax_press "$gpid" checkbox Dashboard >/dev/null || fail "サイドバーの「Dashboard」を押せません"
running_n=$(jq '[.docker.default.Ready.containers[] | select(.state == "running" and (.failed | not))] | length' "$state")
# 枠の中に内訳が出る。枠のボタンの名前は見出し、内訳は枠の中の文字。
wait_until 15 has_element "$gpid" "static text Running: ${running_n}" || fail "コンテナの枠に内訳「Running: ${running_n}」が出ません"
has_prefix "$gpid" "button Containers: " || fail "ダッシュボードにコンテナの枠のボタンがありません"
has_prefix "$gpid" "button Instances: " || fail "ダッシュボードにインスタンスの枠のボタンがありません"
has_element "$gpid" "static text default: Running" || fail "インスタンスの枠に「default: Running」がありません"
shot_rect "en-5d-dashboard" "$(window_rect "$gpid")"
echo "(5d) ダッシュボードのインスタンスの枠とコンテナの枠に内訳が出た（色帯は画像で確かめる）"
row_cleanup
wait_until 30 row_gone || fail "${row_name} を消しても状態ファイルに残ります"
ax_press "$gpid" checkbox Containers >/dev/null || fail "サイドバーの「Containers」を押せません"

# (5b) GUIが開いているときの二度目の「Open Window」は、新しいGUIを立てず、既存のウィンドウを前面に出す。
# choose_menu_item は別のアプリを前面にしてからメニューを開くので、前面に戻れば二度目の起動が働いた証になる。
choose_menu_item "$dpid" "Open Window" "en-5b-menu-open-again"
wait_until 10 is_front "$gpid" || fail "二度目の「Open Window」でGUIが前面に出ません（前面 pid=$(front_pid) $(ps -o args= -p "$(front_pid)" | cut -c1-80)）"
guis_are 1 || fail "二度目の「Open Window」でGUIが2つになりました: $(gui_pids | tr '\n' ' ')"
[ "$(window_count "$gpid")" = 1 ] || fail "ウィンドウが1つではありません: $(window_count "$gpid")"
shot "en-5b-front"
echo "(5b) 二度目の「Open Window」で、GUI pid=$gpid のウィンドウ1つが前面に出た"

# (5c) デーモンを止めると、GUIに停止の表示と「Start daemon」が出る。ボタンで起動し直すと表示が消える。
# GUIが前面のままメニューを開くと、前面の切り替えで閉じることがあるので、先に別のアプリを前面にして落ち着かせる。
timeout 10 osascript -e "tell application \"$front_app\" to activate"
wait_until 10 front_is_not "$gpid" || fail "GUIから前面が移りません"
sleep 1
choose_menu_item "$dpid" "Quit" "en-5c-menu-quit"
wait_until 10 daemons_are 0 || fail "「Quit」でデーモンが終わりません"
# 他のアプリの裏に隠れたウィンドウは描き直されないので、二度目の起動で前面に出してから待つ。
"$exe" gui
wait_until 10 is_front "$gpid" || fail "GUIが前面に出ません"
wait_until 45 has_element "$gpid" "static text The daemon is not running" || fail "デーモン停止の表示がGUIに出ません: $(contents "$gpid" | sed 's/ of window.*//' | head -6 | tr '\n' '|') updated_at=$(jq -c .updated_at "$state") now=$(date +%s) lang=$(ps eww -p "$gpid" | grep -o 'WHARFKEEP_LANG=[a-z]*')"
shot_rect "en-5c-gui-stopped" "$(window_rect "$gpid")"
echo "(5c) デーモンを止めると、GUIに「The daemon is not running」が出た"
ax_press "$gpid" button "Start daemon" >/dev/null || fail "「Start daemon」を押せません"
wait_until 15 daemons_are 1 || fail "「Start daemon」でデーモンが立ちません"
dpid=$(daemon_pids)
wait_until 30 banner_gone "$gpid" || fail "デーモンを起動しても停止の表示が消えません"
echo "(5c) 「Start daemon」でデーモン pid=$dpid が立ち、停止の表示が消えた"

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
echo "(7) 止めた: $(show_state) / アイコン「$(icon_desc "$dpid")」（前は「${before}」）"
choose_menu_item "$dpid" "Start default" "en-7d-menu-start"
wait_until 240 state_default_is Running Ready || fail "default が起動したことが状態ファイルに出ません: $(show_state)"
wait_until 10 icon_is "$dpid" "$before" || fail "アイコンが「${before}」に戻りません: $(icon_desc "$dpid")"
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

# (5) 自動起動をオフにすると、launchdはplistを消したあとデーモンを起こし直さない。動いているデーモンはそのまま動く。
launchctl kickstart "$service"
wait_until 10 launchd_runs || fail "launchdからデーモンを起動できません"
opid=$(launchd_pid)
wait_until 10 status_item_visible "$opid" || fail "launchdのデーモンのメニューバー項目がAXに見えません"
choose_menu_item "$opid" login "autostart-5-menu-off"
wait_until 10 test ! -e "$plist" || fail "オフにしてもplistが消えません"
[ "$(daemon_pids)" = "$opid" ] || fail "オフにしたらデーモンが止まりました"
wait_until 5 status_item_visible "$opid" || fail "オフにしたあとメニューバー項目が消えました"
echo "(autostart-5) オフにするとplistが消え、デーモン pid=$opid とメニューバーは残った"
kill -9 "$opid"
wait_until 10 daemons_are 0 || fail "kill -9 のあとデーモンが残っています"
if wait_until 15 daemons_are 1; then
  fail "オフにしたのにkill -9 のあと起こし直されました"
fi
if launchctl print "$service" >/dev/null 2>&1; then
  fail "オフにしたのにlaunchdにジョブが残っています"
fi
echo "(autostart-6) オフのあと kill -9 しても15秒間起こし直されず、launchdからジョブが外れた"

# (7) オンに戻す。launchdの外で起動したデーモンのメニューから選ぶと、plistが書かれlaunchdに読み込まれる。
WHARFKEEP_LANG=en "$exe" daemon >/dev/null 2>&1 &
wait_until 10 daemons_are 1 || fail "デーモンが起動しません"
mpid=$(daemon_pids)
wait_until 10 status_item_visible "$mpid" || fail "メニューバー項目がAXに見えません"
choose_menu_item "$mpid" login "autostart-7-menu-on"
wait_until 10 launchctl print "$service" >/dev/null 2>&1 || fail "オンに戻してもlaunchdに登録されません"
[ -e "$plist" ] || fail "オンに戻してもplistがありません"
echo "(autostart-7) オンに戻すとplistが書かれ、launchdに登録された"
choose_menu_item "$mpid" last "autostart-7-menu-quit"
wait_until 10 daemons_are 0 || fail "終了でデーモンが終わりません（オンに戻したあと）"

# 削除と片付けの試験。専用のインスタンスの中で、タグ、ボリューム、ネットワーク、ビルドキャッシュを作り、
# 行のごみ箱と「片付け…」から確認ダイアログを通して消す。
echo "== resources"
# 一覧を取れないときに「無い」と見なすと、利用者の同名インスタンスを最後にデータごと消してしまう。
iso_list=$(colima list --json) || fail "colima list に失敗しました"
if printf '%s' "$iso_list" | jq -es --arg n "$iso" 'any(.[]; .name == $n)' >/dev/null; then
  fail "colimaのインスタンス ${iso} が既にあります。試験が消すので、使っていないことを確かめてから colima delete ${iso} してください"
fi
iso_created=1
colima start "$iso" --cpu 1 --memory 2 --disk 10 --vm-type vz >/dev/null 2>&1 || fail "colima start ${iso} に失敗しました"
iso_sock=$(socket_of "$iso") || fail "${iso} のソケットを colima status から取れません"
dki pull -q alpine:latest >/dev/null || fail "${iso} に alpine:latest を取得できません"
dki tag alpine:latest wk-e2e-img:del
dki volume create wk-e2e-vol >/dev/null
anon_vol=$(dki volume create)
dki network create "wk e2e net" >/dev/null
dki network create wk-e2e-unused >/dev/null
# タグなしでビルドすると dangling のイメージになる。層はビルドキャッシュに残る。
printf 'FROM alpine:latest\nRUN echo one > /x\n' | dki build -q - >/dev/null
[ -n "$(dki images -f dangling=true -q)" ] || fail "dangling のイメージを作れません"
# コンテナ画面の見本。実行中、一時停止、停止（0で終了）、失敗（0以外で終了）を1つずつ置く。
# READMEの画像はこの画面を使う。利用者のdefaultのコンテナ名を写さないため。
dki run -d --name web alpine:latest sleep 3600 >/dev/null
dki run -d --name cache alpine:latest sleep 3600 >/dev/null
dki pause cache >/dev/null
dki run --name worker alpine:latest true >/dev/null
dki run --name job alpine:latest sh -c 'exit 3' >/dev/null || true
launchctl kickstart "$service"
wait_until 10 launchd_runs || fail "launchdからデーモンを起動できません（resources）"
wait_until 90 iso_ready || fail "状態ファイルに ${iso} の Ready が出ません"
WHARFKEEP_LANG=en WHARFKEEP_PROFILE=$iso "$exe" gui >/dev/null 2>&1 &
wait_until 10 guis_are 1 || fail "GUIのプロセスが立ちません（resources）"
gpid=$(gui_pids)

# 確認ダイアログを開いて取り消し、もう一度開いて削除する。$1は押す行のボタン、$2は取り消しで残ると確かめる条件、
# $3は削除で消えると確かめる条件。条件は空白入りの名前を引用符で書けるよう eval で評価する。
delete_via_row() {
  local button=$1 stays=$2 gone=$3 shotname=$4
  wait_until 20 has_element "$gpid" "button $button" || fail "「${button}」がAXで見えません"
  ax_press "$gpid" button "$button" >/dev/null || fail "「${button}」を押せません"
  wait_until 10 has_element "$gpid" "button Cancel" || fail "「${button}」の確認ダイアログが出ません"
  shot_rect "$shotname" "$(window_rect "$gpid")"
  ax_press "$gpid" button Cancel >/dev/null || fail "確認ダイアログの「Cancel」を押せません"
  wait_until 5 has_element "$gpid" "button $button" || fail "取り消したあと行が見えません"
  sleep 1
  eval "$stays" || fail "「Cancel」で取り消したのに消えました: ${button}"
  ax_press "$gpid" button "$button" >/dev/null || fail "「${button}」を押せません（2度目）"
  wait_until 10 has_element "$gpid" "button Cancel" || fail "2度目の確認ダイアログが出ません"
  ax_press "$gpid" button Delete >/dev/null || fail "確認ダイアログの「Delete」を押せません"
  wait_until 60 eval "$gone" || fail "「Delete」を押しても消えません: ${button}"
}

wait_until 20 has_element "$gpid" "checkbox Containers" || fail "サイドバーの「Containers」がAXで見えません（resources）"
ax_press "$gpid" checkbox Containers >/dev/null
wait_until 15 has_element "$gpid" "button Stop web" || fail "見本の行「Stop web」がAXで見えません"
wait_until 15 has_element "$gpid" "button Start job" || fail "見本の行「Start job」がAXで見えません"
for s in Running Paused Stopped Failed; do
  has_element "$gpid" "static text $s" || fail "見本の行に状態の文字「${s}」がありません"
done
shot_rect "en-9-containers" "$(window_rect "$gpid")"
echo "(9) 見本のコンテナ4つが、実行中・一時停止・停止・失敗の状態で並んだ"
ax_press "$gpid" checkbox Dashboard >/dev/null || fail "サイドバーの「Dashboard」を押せません（resources）"
wait_until 15 has_element "$gpid" "static text Failed: 1" || fail "見本のダッシュボードに「Failed: 1」が出ません"
shot_rect "en-9-dashboard" "$(window_rect "$gpid")"

# イメージ: 行のごみ箱でタグを消す（イメージ本体の alpine は残る）。
wait_until 20 has_element "$gpid" "checkbox Images" || fail "サイドバーの「Images」がAXで見えません"
ax_press "$gpid" checkbox Images >/dev/null
wait_until 15 has_element "$gpid" "button Delete wk-e2e-img:del" || fail "イメージの行がAXで見えません"
shot_rect "en-9a-images" "$(window_rect "$gpid")"
delete_via_row "Delete wk-e2e-img:del" "img_has wk-e2e-img:del" "img_gone wk-e2e-img:del" "en-9b-confirm-image"
img_has alpine:latest || fail "タグを消したら alpine:latest まで消えました"
# 本体の alpine は残るので、空いた容量ではなくタグを外した知らせが出る。
wait_until 15 has_element "$gpid" "static text Removed tag wk-e2e-img:del" || fail "タグを外した通知が出ません"
echo "(9) イメージの行のごみ箱: 取り消しで残り、削除でタグ wk-e2e-img:del が消え、その通知が出た"

# イメージ: 「片付け…」で dangling とビルドキャッシュを消す。
ax_press "$gpid" button "Clean up…" >/dev/null || fail "「Clean up…」を押せません"
wait_until 10 has_element "$gpid" "radio button Remove dangling images only" || fail "片付けのダイアログが出ません"
cache_name=$(elem_name "$gpid" checkbox "Remove the build cache")
[ -n "$cache_name" ] || fail "ビルドキャッシュの選択肢がありません"
ax_press "$gpid" "radio button" "Remove dangling images only" >/dev/null || fail "dangling の選択肢を押せません"
ax_press "$gpid" checkbox "$cache_name" >/dev/null || fail "「${cache_name}」を押せません"
shot_rect "en-9c-cleanup-images" "$(window_rect "$gpid")"
ax_press "$gpid" button Delete >/dev/null || fail "片付けの「Delete」を押せません"
wait_until 90 no_dangling || fail "片付けのあとも dangling のイメージが残ります"
wait_until 60 cache_empty || fail "片付けのあともビルドキャッシュが残ります: $(dki system df)"
wait_until 15 has_prefix "$gpid" "static text Freed " || fail "空いた容量の通知が出ません"
shot_rect "en-9d-freed-toast" "$(window_rect "$gpid")"
echo "(9) イメージの「片付け…」: dangling とビルドキャッシュが消え、空いた容量の通知が出た"

# ボリューム
ax_press "$gpid" checkbox Volumes >/dev/null || fail "サイドバーの「Volumes」を押せません"
wait_until 15 has_element "$gpid" "button Delete wk-e2e-vol" || fail "ボリュームの行がAXで見えません"
shot_rect "en-9e-volumes" "$(window_rect "$gpid")"
delete_via_row "Delete wk-e2e-vol" "vol_has wk-e2e-vol" "vol_gone wk-e2e-vol" "en-9f-confirm-volume"
ax_press "$gpid" button "Clean up…" >/dev/null || fail "ボリュームの「Clean up…」を押せません"
wait_until 10 has_prefix "$gpid" "static text Delete 1 unused anonymous volumes" || fail "未使用ボリュームの件数が確認ダイアログに出ません"
shot_rect "en-9g-cleanup-volumes" "$(window_rect "$gpid")"
ax_press "$gpid" button Delete >/dev/null || fail "ボリュームの片付けの「Delete」を押せません"
wait_until 60 no_volumes || fail "片付けのあともボリュームが残ります: $(dki volume ls -q | tr '\n' ' ')"
echo "(9) ボリュームの行のごみ箱と「片付け…」で wk-e2e-vol と無名ボリュームが消えた"

# ネットワーク。名前に空白があるので、IDで消せることも確かめる。
ax_press "$gpid" checkbox Networks >/dev/null || fail "サイドバーの「Networks」を押せません"
wait_until 15 has_element "$gpid" "button Delete wk e2e net" || fail "ネットワークの行がAXで見えません"
has_element "$gpid" "button Delete bridge" && fail "既定の bridge に削除ボタンがあります"
ax_press "$gpid" button "Inspect bridge" >/dev/null || fail "「Inspect bridge」を押せません"
wait_until 10 has_prefix "$gpid" "static text Inspect: bridge" || fail "詳細の見出しが出ません"
shot_rect "en-9h-networks" "$(window_rect "$gpid")"
delete_via_row "Delete wk e2e net" "net_has 'wk e2e net'" "net_gone 'wk e2e net'" "en-9i-confirm-network"
ax_press "$gpid" button "Clean up…" >/dev/null || fail "ネットワークの「Clean up…」を押せません"
wait_until 10 has_prefix "$gpid" "static text Delete all unused networks" || fail "ネットワークの片付けの確認が出ません"
ax_press "$gpid" button Delete >/dev/null || fail "ネットワークの片付けの「Delete」を押せません"
wait_until 60 net_gone wk-e2e-unused || fail "片付けのあとも wk-e2e-unused が残ります"
net_has bridge || fail "既定の bridge が消えました"
echo "(9) ネットワークの行のごみ箱（空白入りの名前）と「片付け…」で消え、bridge は残った"
close_window "$gpid"

# 日本語の画面を1枚撮る。
dki tag alpine:latest wk-e2e-ja:1
wait_until 30 jq -e --arg n "$iso" '.docker[$n].Ready.images[]? | select(.repo_tags | index("wk-e2e-ja:1"))' "$state" >/dev/null || fail "状態ファイルに wk-e2e-ja:1 が出ません"
WHARFKEEP_LANG=ja WHARFKEEP_PROFILE=$iso "$exe" gui >/dev/null 2>&1 &
wait_until 10 guis_are 1 || fail "GUIのプロセスが立ちません（resources ja）"
gpid=$(gui_pids)
wait_until 20 has_element "$gpid" "checkbox イメージ" || fail "サイドバーの「イメージ」がAXで見えません"
ax_press "$gpid" checkbox イメージ >/dev/null
wait_until 15 has_element "$gpid" "button 削除 wk-e2e-ja:1" || fail "日本語のイメージの行がAXで見えません"
shot_rect "ja-9-images" "$(window_rect "$gpid")"
close_window "$gpid"
echo "(9) 日本語のイメージ画面を撮った"

# 日本語の文言が描けることを確かめる。メニューバーの流れは上で確かめたのでGUIだけを開く。
echo "== ja"
colima_running || fail "日本語の確かめの前にcolimaが動いていません"
# 行を出すにはデーモンが状態ファイルを書く必要がある。
launchctl kickstart "$service"
wait_until 10 launchd_runs || fail "launchdからデーモンを起動できません（ja）"
row_up
WHARFKEEP_LANG=ja "$exe" gui >/dev/null 2>&1 &
wait_until 10 guis_are 1 || fail "GUIのプロセスが立ちません（ja）"
gpid=$(gui_pids)
check_navigation "$gpid" "コンテナ" "ダッシュボード"
wait_until 15 has_element "$gpid" "button 停止 ${row_name}" || fail "日本語の行の「停止 ${row_name}」がAXで見えません"
has_element "$gpid" "static text 実行中" || fail "日本語の行に状態の文字「実行中」がありません"
shot_rect "ja-5-gui" "$(window_rect "$gpid")"
close_window "$gpid"
row_cleanup
echo "(ja) 「コンテナ」を押し、見出しが「コンテナ」になり、閉じるとGUIが終わった"

launchctl kickstart "$service"
wait_until 10 launchd_runs || fail "launchdからデーモンを起動し直せません"
grep -q 'state = running' <<<"$(launchctl print "$service")" || fail "終わりにlaunchdのデーモンが running ではありません"
[ -f "$plist" ] || fail "終わりにplistがありません"
echo "(restore) launchdのデーモン pid=$(launchd_pid) が running で、plistが残ったまま終える"

succeeded=1
echo "e2e: 成功（画面は ${out}）"
