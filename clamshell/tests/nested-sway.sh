#!/bin/sh
# The launcher under sway, end to end: runs a headless sway (two outputs, the Thor's screens at half
# size; nothing shows on the desktop, and it needs no display or input devices), the system program
# inside it with a test library and test apps, drives it through its control pipe (CLAMSHELL_CONTROL: the
# taps and long presses a user would make), and checks sway's window tree and systemd's units. Leaves
# nothing behind: its instances are stopped and sway is closed at the end. Needs sway, foot, sqlite3 and
# a release build.
#
#   clamshell/tests/nested-sway.sh            (exit status: the number of failed checks)
set -u
HERE=$(cd "$(dirname "$0")/.." && pwd)
BIN=$HERE/target/release/clamshell
T=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/clamshell-test.XXXXXX")
FAILED=0

pass() { echo "  ok    $1"; }
fail() { echo "  FAIL  $1"; FAILED=$((FAILED + 1)); }
check() { if eval "$2"; then pass "$1"; else fail "$1"; fi; }
# Waits up to $2 seconds for a condition.
until_() { n=0; while ! eval "$1"; do n=$((n + 1)); [ $n -ge $(( $2 * 10 )) ] && return 1; sleep 0.1; done; }

# --- the test environment
mkdir -p "$T/config/clamshell" "$T/pegasus" "$T/games" "$T/state"
cat > "$T/config/clamshell/launcher.json" <<'EOF'
{ "apps": [
  { "name": "Shelf", "icon": "shelf", "builtin": "shelf" },
  { "name": "Terminal", "icon": "terminal", "command": ["foot", "sleep", "300"], "multiple": true },
  { "name": "Slow", "command": ["sh", "-c", "sleep 3; exec foot --app-id=slow sleep 300"] },
  { "name": "Spawner", "command": ["sh", "-c", "foot --app-id=first sleep 300 & sleep 2; foot --app-id=late sleep 300 & wait"] },
  { "name": "Detached", "command": ["sh", "-c", "exec env -u XDG_ACTIVATION_TOKEN -u DESKTOP_STARTUP_ID setsid -f foot --app-id=detached sleep 300; sleep 300"] }
] }
EOF
echo "$T/games" > "$T/pegasus/game_dirs.txt"
cat > "$T/games/metadata.pegasus.txt" <<'EOF'
collection: Test
extension: game
launch: sh -c "foot --app-id=testgame sleep 300 & sleep 300"

game: Test Game
file: test.game
EOF
touch "$T/games/test.game"
cat > "$T/sway.conf" <<'EOF'
default_border none
default_floating_border none
output HEADLESS-1 resolution 960x540 position 0 0
output HEADLESS-2 resolution 620x540 position 960 0
EOF
UNITS_BEFORE=$(systemctl --user list-units 'clamshell-*' --plain --no-legend | awk '{print $1}')

mkfifo "$T/control"
env -u WAYLAND_DISPLAY -u DISPLAY WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=2 WLR_RENDERER=pixman sway -c "$T/sway.conf" > "$T/sway.log" 2>&1 &
SWAY=$!
until_ "[ -S /run/user/$(id -u)/sway-ipc.$(id -u).$SWAY.sock ]" 10 || { echo "sway didn't start"; exit 99; }
SOCK=/run/user/$(id -u)/sway-ipc.$(id -u).$SWAY.sock
msg() { swaymsg -s "$SOCK" "$@" > /dev/null; }
json() { swaymsg -s "$SOCK" -t "$1" | python3 -c "$2"; }
front() { json get_workspaces "import json,sys; print(next((w['name'] for w in json.load(sys.stdin) if w['focused']), ''))"; }
# The workspace of the (first) window with this app id, and that workspace's output.
ws_of() { json get_tree "
import json,sys
def walk(n, ws=''):
    if n.get('type') == 'workspace': ws = n['name']
    if n.get('app_id') == '$1' or (n.get('window_properties') or {}).get('class') == '$1': print(ws); sys.exit()
    for k in ('nodes', 'floating_nodes'):
        for c in n.get(k, []): walk(c, ws)
walk(json.load(sys.stdin))"; }
output_of() { json get_workspaces "import json,sys; print(next((w['output'] for w in json.load(sys.stdin) if w['name'] == '$1'), ''))"; }
units() { systemctl --user list-units "clamshell-$1*" --plain --no-legend | awk '{print $1}' | sed 's/\.scope$//'; }
# A tap or long press, as the program's control pipe takes them.
ctl() { echo "$*" > "$T/control"; sleep 0.3; }
start_program() {
    msg exec "env CLAMSHELL_TEST=$T CLAMSHELL_CONTROL=$T/control CLAMSHELL_STEAM=off CLAMSHELL_PEGASUS_CONFIG=$T/pegasus XDG_CONFIG_HOME=$T/config XDG_STATE_HOME=$T/state CLAMSHELL_BOTTOM_OUTPUT=HEADLESS-2 CLAMSHELL_START_MODE=launcher $BIN >> $T/program.log 2>&1"
}
program_pid() { for p in $(pgrep -x clamshell); do tr '\0' '\n' < "/proc/$p/environ" 2>/dev/null | grep -qx "CLAMSHELL_TEST=$T" && echo "$p"; done; }
# The launcher's apps, by position in launcher.json.
SHELF=0; TERMINAL=1; SLOW=2; SPAWNER=3; DETACHED=4

echo "nested sway: $SOCK"
start_program
until_ '[ "$(ws_of clamshell.top)" = clamshell-shelf ]' 10
check "start: shelf on its workspace, in front" '[ "$(ws_of clamshell.top)" = clamshell-shelf ] && [ "$(front)" = clamshell-shelf ]'
check "start: the second screen is a layer surface on HEADLESS-2" 'grep -q "layer surface on HEADLESS-2" "$T/program.log"'

ctl tap app $TERMINAL
until_ '[ -n "$(units app-terminal)" ] && [ "$(front)" = "$(units app-terminal | head -1)" ]' 5
T1=$(units app-terminal | head -1)
check "tap Terminal: an instance (scope), shown when its window is there" '[ -n "$T1" ] && [ "$(ws_of foot)" = "$T1" ] && [ "$(front)" = "$T1" ]'

ctl tap plus $TERMINAL
until_ '[ "$(units app-terminal | wc -l)" = 2 ] && [ "$(front)" != "$T1" ]' 5
T2=$(front)
check "+: a second instance, in front" '[ "$(units app-terminal | wc -l)" = 2 ] && [ "$T2" != "$T1" ] && [ "${T2#clamshell-app-terminal-}" != "$T2" ]'
ctl tap app $TERMINAL; sleep 0.5
check "tap Terminal while one is in front: the next" '[ "$(front)" = "$T1" ]'
check "our workspaces are on the main screen" '[ "$(output_of "$T1")" = HEADLESS-1 ] && [ "$(output_of "$T2")" = HEADLESS-1 ]'

ctl tap app $SLOW; sleep 0.5; ctl tap app $SHELF; sleep 1
check "a slow app: its empty workspace isn't shown meanwhile" '[ "$(front)" = clamshell-shelf ]'
until_ '[ -n "$(ws_of slow)" ]' 8
sleep 0.5
check "a slow app: its window on its workspace, brought to the front" '[ "$(ws_of slow)" = "$(units app-slow)" ] && [ "$(front)" = "$(units app-slow)" ]'

ctl tap app $SPAWNER
until_ '[ -n "$(ws_of late)" ]' 8; sleep 0.5
check "a later window of a launch: on the launch's workspace" '[ "$(ws_of first)" = "$(units app-spawner)" ] && [ "$(ws_of late)" = "$(units app-spawner)" ]'

ctl tap app $DETACHED
until_ '[ -n "$(ws_of detached)" ]' 8; sleep 0.5
check "a window sway can't trace (detached, no token): on its instance's workspace, by its scope" '[ "$(ws_of detached)" = "$(units app-detached)" ]'

msg 'focus output HEADLESS-2'; sleep 0.3; msg exec "foot --app-id=stray sleep 300"
until_ '[ -n "$(ws_of stray)" ]' 5; sleep 0.5
check "a window opening on the second screen goes to the main screen" '[ "$(output_of "$(ws_of stray)")" = HEADLESS-1 ]'
msg '[app_id="^stray$"] kill'

msg exec "foot --app-id=steam_app_42 sleep 300"
until_ '[ -n "$(ws_of steam_app_42)" ]' 5
check "a Steam game's window goes to the game's workspace" '[ "$(ws_of steam_app_42)" = clamshell-game ]'
msg '[app_id="^steam_app_42$"] kill'; sleep 0.5

ctl tap app $SHELF; sleep 0.5; ctl tap launch
until_ '[ "$(front)" = clamshell-game ]' 8
check "a game from shelf: on the game's workspace, in front" '[ "$(ws_of testgame)" = clamshell-game ] && [ "$(front)" = clamshell-game ] && [ -n "$(units game)" ]'
ctl long game
until_ '[ -z "$(units game)" ]' 10
check "long press on the game: closed, its scope killed after 5 s" '[ -z "$(units game)" ]'
check "  and back to shelf" '[ "$(front)" = clamshell-shelf ]'
plays() { [ -f "$T/pegasus/stats.db" ] && sqlite3 "$T/pegasus/stats.db" "select count(*) from plays" 2> /dev/null || echo 0; }
until_ '[ "$(plays)" -ge 1 ]' 5
check "  and the play recorded" '[ "$(plays)" -ge 1 ]'

ctl tap app $TERMINAL; sleep 0.5; FRONT=$(front)
ctl long app $TERMINAL
until_ '[ "$(units app-terminal | wc -l)" = 1 ]' 5
check "long press on Terminal: closes the instance it is on" '[ "$(units app-terminal | wc -l)" = 1 ] && ! units app-terminal | grep -qx "$FRONT"'

kill "$(program_pid)"; sleep 1; start_program
until_ '[ -n "$(program_pid)" ] && [ "$(front)" = clamshell-shelf ]' 10
check "restart: shelf in front again" '[ "$(front)" = clamshell-shelf ] && [ "$(ws_of clamshell.top)" = clamshell-shelf ]'
check "restart: the instances still run" '[ -n "$(units app-spawner)" ] && [ -n "$(units app-detached)" ] && [ -n "$(units app-terminal)" ]'
ctl tap app $TERMINAL; sleep 0.5
check "restart: a tap finds the running instance" '[ "$(front)" = "$(units app-terminal)" ]'

# --- clean up: the program, this test's scopes, sway
kill "$(program_pid)" 2> /dev/null
for u in $(systemctl --user list-units 'clamshell-*' --plain --no-legend | awk '{print $1}'); do
    echo "$UNITS_BEFORE" | grep -qx "$u" || systemctl --user stop "$u"
done
kill $SWAY; wait $SWAY 2> /dev/null
rm -rf "${T:?}"
echo "$FAILED failed"
exit $FAILED
