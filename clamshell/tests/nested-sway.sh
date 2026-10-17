#!/bin/sh
# The dock under sway, end to end: runs a headless sway (two outputs, the Thor's screens at half
# size; nothing shows on the desktop, and it needs no display or input devices), ClamShell
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
# A check of something that is coming: waits for it (up to $3 seconds, default 5), then checks.
check_soon() { until_ "$2" "${3:-5}"; check "$1" "$2"; }

# --- the test environment
mkdir -p "$T/config/clamshell" "$T/pegasus" "$T/games" "$T/state"
cat > "$T/config/clamshell/launcher.json" <<'EOF'
{ "apps": [
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

game: Zeta Game
file: zeta.game
launch: sh -c "foot --app-id=zetagame sleep 300 & sleep 300"
EOF
touch "$T/games/test.game" "$T/games/zeta.game"
# The outputs, and how a ClamShell session behaves (as on the device).
cat > "$T/sway.conf" <<EOF
include $HERE/sway/clamshell.conf
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
# A dock app's workspace: all its instances' windows, as tabs.
space() { echo "clamshell-app-$1"; }
# How many windows with this app id are on a workspace; the focused window's id.
count_on() { json get_tree "
import json,sys
n=0
def walk(c, ws=''):
    global n
    if c.get('type') == 'workspace': ws = c['name']
    if ws == '$1' and c.get('app_id') == '$2': n += 1
    for k in ('nodes', 'floating_nodes'):
        for d in c.get(k, []): walk(d, ws)
walk(json.load(sys.stdin)); print(n)"; }
focused_id() { json get_tree "
import json,sys
def walk(c):
    if c.get('focused'): print(c['id']); sys.exit()
    for k in ('nodes', 'floating_nodes'):
        for d in c.get(k, []): walk(d)
walk(json.load(sys.stdin))"; }
# A tap or long press, as the program's control pipe takes them.
ctl() { echo "$*" > "$T/control"; sleep 0.3; }
start_program() {
    msg exec "env CLAMSHELL_TEST=$T CLAMSHELL_CONTROL=$T/control CLAMSHELL_STEAM=off CLAMSHELL_PEGASUS_CONFIG=$T/pegasus XDG_CONFIG_HOME=$T/config XDG_STATE_HOME=$T/state CLAMSHELL_BOTTOM_OUTPUT=HEADLESS-2 CLAMSHELL_START_MODE=dock $BIN >> $T/program.log 2>&1"
}
program_pid() { for p in $(pgrep -x clamshell); do { tr '\0' '\n' < "/proc/$p/environ"; } 2>/dev/null | grep -qx "CLAMSHELL_TEST=$T" && echo "$p"; done; }
# The launcher's apps, by position in launcher.json.
TERMINAL=0; SLOW=1; SPAWNER=2; DETACHED=3

echo "nested sway: $SOCK"
start_program
until_ '[ "$(ws_of clamshell.top)" = clamshell-shelf ]' 10
check "start: shelf on its workspace, in front" '[ "$(ws_of clamshell.top)" = clamshell-shelf ] && [ "$(front)" = clamshell-shelf ]'
check_soon "start: the second screen is a layer surface on HEADLESS-2" 'grep -q "layer surface on HEADLESS-2" "$T/program.log"'

TS=$(space terminal)
ctl tap app $TERMINAL
until_ '[ -n "$(units app-terminal)" ] && [ "$(front)" = "$TS" ]' 5
check "tap Terminal: an instance (scope), its window on the app's workspace, shown when it is there" '[ -n "$(units app-terminal)" ] && [ "$(ws_of foot)" = "$TS" ] && [ "$(front)" = "$TS" ]'

ctl tap plus $TERMINAL
until_ '[ "$(units app-terminal | wc -l)" = 2 ] && [ "$(count_on "$TS" foot)" = 2 ]' 5
check "+: a second instance, a tab of the same workspace, in front" '[ "$(units app-terminal | wc -l)" = 2 ] && [ "$(count_on "$TS" foot)" = 2 ] && [ "$(front)" = "$TS" ]'
before=$(focused_id)
ctl tap app $TERMINAL
check_soon "tap Terminal while it is in front: it stays as it is" '[ "$(focused_id)" = "$before" ] && [ "$(front)" = "$TS" ]'
check "our workspaces are on the main screen" '[ "$(output_of "$TS")" = HEADLESS-1 ]'

ctl tap app $SLOW; sleep 0.5; msg 'workspace clamshell-shelf'; sleep 1
check "a slow app: its empty workspace isn't shown meanwhile" '[ "$(front)" = clamshell-shelf ]'
check_soon "a slow app: its window on its workspace, brought to the front" '[ "$(ws_of slow)" = "$(space slow)" ] && [ "$(front)" = "$(space slow)" ]' 8

ctl tap app $SPAWNER
check_soon "a later window of a launch: on the app's workspace" '[ -n "$(ws_of late)" ] && [ "$(ws_of first)" = "$(space spawner)" ] && [ "$(ws_of late)" = "$(space spawner)" ]' 8

# The bottom screen: the spawner's windows moved down (Home+Down) and up again (Home+Up).
SP=$(space spawner)
RULES="$T/pegasus/window-rules.txt"
focused_output() { json get_outputs "import json,sys; print(next((o['name'] for o in json.load(sys.stdin) if o['focused']), ''))"; }
msg "[app_id=\"^late$\"] focus"; sleep 0.3
ctl window down
check_soon "Home+Down: the window in front to the bottom screen; the main screen keeps the keyboard" \
    '[ "$(output_of "$(ws_of late)")" = HEADLESS-2 ] && [ "$(ws_of first)" = "$SP" ] && [ "$(focused_output)" = HEADLESS-1 ]'
check "  remembered for its program" 'grep -q "^bottom	app	spawner	late	" "$RULES"'
msg "[app_id=\"^first$\"] focus"; sleep 0.3
ctl window down
check_soon "  its last window down too: its workspace empty, shelf in front" '[ "$(output_of "$(ws_of first)")" = HEADLESS-2 ] && [ "$(front)" = clamshell-shelf ]'
ctl window up; sleep 0.3; ctl window up
check_soon "Home+Up, twice: both back on their workspace, the rules forgotten" '[ "$(ws_of late)" = "$SP" ] && [ "$(ws_of first)" = "$SP" ] && ! grep -q "^bottom" "$RULES"'

ctl tap app $DETACHED
check_soon "a window sway can't trace (detached, no token): on its app's workspace, by its scope" '[ -n "$(ws_of detached)" ] && [ "$(ws_of detached)" = "$(space detached)" ]' 8

msg 'focus output HEADLESS-2'; sleep 0.3; msg exec "foot --app-id=stray sleep 300"
check_soon "a window opening on the second screen goes to the main screen" '[ -n "$(ws_of stray)" ] && [ "$(output_of "$(ws_of stray)")" = HEADLESS-1 ]'
msg '[app_id="^stray$"] kill'

msg exec "foot --app-id=steam_app_42 sleep 300"
until_ '[ -n "$(ws_of steam_app_42)" ]' 5
check "a Steam game's window goes to the game's workspace" '[ "$(ws_of steam_app_42)" = clamshell-game ]'
msg '[app_id="^steam_app_42$"] kill'; until_ '[ -z "$(ws_of steam_app_42)" ]' 5

msg 'workspace clamshell-shelf'; until_ '[ "$(front)" = clamshell-shelf ]' 5; ctl tap launch
until_ '[ -n "$(units game)" ] && [ -n "$(ws_of testgame)" ] && [ "$(front)" = "$(units game | head -1)" ]' 8
G1=$(units game | head -1)
check "a game from shelf: on its own workspace, in front" '[ -n "$G1" ] && [ "$(ws_of testgame)" = "$G1" ] && [ "$(front)" = "$G1" ]'
# A second game while it runs: shelf offers to keep the first running.
msg 'workspace clamshell-shelf'; until_ '[ "$(front)" = clamshell-shelf ]' 5; ctl list 1; ctl tap launch; sleep 0.3; ctl tap keep
until_ '[ "$(units game | wc -l)" = 2 ] && [ -n "$(ws_of zetagame)" ] && [ "$(front)" = "$(ws_of zetagame)" ]' 8
check "a second game kept beside the first: both run, each on its workspace, the new one in front" \
    '[ "$(units game | wc -l)" = 2 ] && [ "$(ws_of zetagame)" != "$G1" ] && [ "$(ws_of testgame)" = "$G1" ] && [ "$(front)" = "$(ws_of zetagame)" ]'
ctl tap game 0
check_soon "the dock's first game tile shows the first game" '[ "$(front)" = "$G1" ]'
ctl long game 1
until_ '[ "$(units game | wc -l)" = 1 ]' 10
ctl long game 0
until_ '[ -z "$(units game)" ]' 10
check "long press on each game: closed, their scopes killed after 5 s" '[ -z "$(units game)" ]'
check "  and back to shelf" '[ "$(front)" = clamshell-shelf ]'
plays() { [ -f "$T/pegasus/stats.db" ] && sqlite3 "$T/pegasus/stats.db" "select count(*) from plays" 2> /dev/null || echo 0; }
until_ '[ "$(plays)" -ge 2 ]' 5
check "  and both plays recorded" '[ "$(plays)" -ge 2 ]'

# The scope of the focused window's process (the instance whose tab is in view).
focused_scope() { pid=$(json get_tree "
import json,sys
def walk(c):
    if c.get('focused'): print(c.get('pid', '')); sys.exit()
    for k in ('nodes', 'floating_nodes'):
        for d in c.get(k, []): walk(d)
walk(json.load(sys.stdin))"); grep -o 'clamshell-app-[^/]*\.scope' "/proc/$pid/cgroup" 2> /dev/null | sed 's/\.scope$//'; }
ctl tap app $TERMINAL; until_ '[ "$(front)" = "$TS" ]' 5; FRONT=$(focused_scope)
ctl long app $TERMINAL
until_ '[ "$(units app-terminal | wc -l)" = 1 ]' 5
check "long press on Terminal: closes the instance whose tab is in view" '[ -n "$FRONT" ] && [ "$(units app-terminal | wc -l)" = 1 ] && ! units app-terminal | grep -qx "$FRONT"'

kill "$(program_pid)"; until_ '[ -z "$(program_pid)" ]' 5; start_program
until_ '[ -n "$(program_pid)" ] && [ "$(front)" = clamshell-shelf ]' 10
check "restart: shelf in front again" '[ "$(front)" = clamshell-shelf ] && [ "$(ws_of clamshell.top)" = clamshell-shelf ]'
check "restart: the instances still run" '[ -n "$(units app-spawner)" ] && [ -n "$(units app-detached)" ] && [ -n "$(units app-terminal)" ]'
ctl tap app $TERMINAL
check_soon "restart: a tap shows the running app's workspace" '[ "$(front)" = "$TS" ]'

# --- clean up: the program, this test's scopes, sway
kill "$(program_pid)" 2> /dev/null
for u in $(systemctl --user list-units 'clamshell-*' --plain --no-legend | awk '{print $1}'); do
    echo "$UNITS_BEFORE" | grep -qx "$u" || systemctl --user stop "$u"
done
kill $SWAY; wait $SWAY 2> /dev/null
rm -rf "${T:?}"
echo "$FAILED failed"
exit $FAILED
