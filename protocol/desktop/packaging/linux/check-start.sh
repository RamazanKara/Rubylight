#!/usr/bin/env bash
# Starts Nightjar under Xvfb and checks that the launcher window comes up and stays up, not SDL's
# error box (SDL fails to start without its X11 and ALSA libraries, which `--version` never
# loads). The release workflow runs it in clean containers that hold only the package and its
# hard dependencies; the window's class must be the desktop entry's StartupWMClass.
#
#   check-start.sh <command that starts nightjar>...
#
# Needs Xvfb, xwininfo and xprop.
set -euo pipefail
[[ $# -gt 0 ]] || { echo "usage: check-start.sh <command>..." >&2; exit 64; }
export NIGHTJAR_CONFIG_DIR=${NIGHTJAR_CONFIG_DIR:-$(mktemp -d)}
export DISPLAY=:7
Xvfb "$DISPLAY" -screen 0 1920x1080x24 -nolisten tcp >/dev/null 2>&1 &
xvfb=$!
trap 'kill "$xvfb" 2>/dev/null || true' EXIT
for _ in $(seq 50); do xwininfo -root >/dev/null 2>&1 && break; sleep 0.1; done
out=$NIGHTJAR_CONFIG_DIR/start.log
log=$NIGHTJAR_CONFIG_DIR/logs/nightjar.log
"$@" >"$out" 2>&1 &
app=$!
sleep 10
fail() {
	echo "error: $1" >&2
	tail -n 20 "$out" "$log" >&2 2>/dev/null || true
	exit 1
}
kill -0 "$app" 2>/dev/null || fail "nightjar exited within 10 s"
if grep -qi 'could not start' "$out" "$log" 2>/dev/null; then fail "SDL did not start"; fi
window=$(xwininfo -root -tree | grep '"Nightjar"' | grep -oE '0x[0-9a-f]+' | head -n1 || true)
[[ -n $window ]] || fail "no Nightjar window"
height=$(xwininfo -id "$window" | sed -n 's/.*Height: *//p')
class=$(xprop -id "$window" WM_CLASS)
[[ $height -ge 300 ]] || fail "the window is $height pixels high: an error box, not the launcher"
[[ $class == *'"io.github.ramazankara.nightjar"'* ]] || fail "the window's class is not the desktop entry's StartupWMClass: $class"
echo "started: window $window, $height pixels high, $class"
kill "$app"
wait "$app" 2>/dev/null || true
