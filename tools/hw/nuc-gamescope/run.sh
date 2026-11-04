#!/bin/bash
# run.sh <name> <xvfb|gs|gshdr|fp|fphdr> <hdr 0|1> <auto|launcher>: one Nightjar run in the
# nj-gamescope container, as the user. auto: a 60 s autostream of Desktop from the isolated host
# ($HOST); launcher: no autostream, pad.py (run as root beside it) drives the launcher. gs and
# gshdr run the built binary under headless gamescope 1280x800@60 (gshdr with --hdr-enabled and
# $GSX), xvfb outside gamescope; fp and fphdr run the installed Flatpak (flatpak.sh) the same way,
# with $FPARGS for `flatpak run`. Results in $W/runs/<name>/: stdout/stderr, nightjar.log,
# timing.csv, and window.json before and after (gamescope must leave the saved place alone).
set -u
name=$1 mode=$2 hdr=$3 kind=$4
W=${W:-$HOME/deck} HOST=${HOST:-192.168.4.69:48463} CFG=${CFG:-$HOME/nj440/cfg}
r=$W/runs/$name bin=${BIN:-$W/target-arch/release/nightjar}
app=io.github.ramazankara.nightjar
rm -rf $r && mkdir -p $r
case $mode in
fp*) export HOME=$W/fphome FLATPAK_USER_DIR=$W/fphome/flatpak; cfg=$HOME/.var/app/$app/config/nightjar ;;
*) cfg=$r/cfg ;;
esac
rm -rf $cfg && mkdir -p $(dirname $cfg) && cp -a $CFG $cfg && rm -rf $cfg/logs
cp $cfg/window.json $r/window.before.json 2>/dev/null
python -c "
import json
p='$cfg/settings.json'; d=json.load(open(p))
d.update(width=1280,height=800,fps=60,hdr=bool($hdr),overlay=False)
json.dump(d,open(p,'w'),indent=2)"
# Flatpak shares xdg-run paths from /run/user/<uid>, whatever XDG_RUNTIME_DIR says (as on a Deck,
# where both are the same): the Flatpak runs use that folder (container.sh setup makes it).
case $mode in
fp*) export XDG_RUNTIME_DIR=/run/user/$(id -u) && find $XDG_RUNTIME_DIR -mindepth 1 -delete ;;
*) export XDG_RUNTIME_DIR=/tmp/xdg-$name && rm -rf $XDG_RUNTIME_DIR && mkdir -m 700 -p $XDG_RUNTIME_DIR ;;
esac
export SDL_AUDIO_DRIVER=dummy
case $mode in
fp*) export NIGHTJAR_TIMING_CSV=$HOME/.var/app/$app/timing.csv; unset NIGHTJAR_CONFIG_DIR; rm -f $NIGHTJAR_TIMING_CSV
  run=(flatpak run ${FPARGS:-} $app) ;;
*) export NIGHTJAR_CONFIG_DIR=$cfg NIGHTJAR_TIMING_CSV=$r/timing.csv
  run=($bin) ;;
esac
if [ $kind = auto ]; then
  export NIGHTJAR_AUTOSTREAM=$HOST NIGHTJAR_AUTOSTREAM_APP=Desktop NIGHTJAR_AUTOSTREAM_PIN=1234 NIGHTJAR_AUTOSTREAM_SECONDS=60
fi
gs=(gamescope --backend headless -W 1280 -H 800 -r 60)
case $mode in
gs | fp) timeout ${T:-150} "${gs[@]}" -- "${run[@]}" >$r/stdout.log 2>$r/stderr.log ;;
gshdr | fphdr) timeout ${T:-150} "${gs[@]}" --hdr-enabled ${GSX:-} -- "${run[@]}" >$r/stdout.log 2>$r/stderr.log ;;
xvfb)
  Xvfb :37 -screen 0 1280x800x24 -nolisten tcp >$r/xvfb.log 2>&1 & xv=$!
  sleep 2
  DISPLAY=:37 timeout ${T:-150} "${run[@]}" >$r/stdout.log 2>$r/stderr.log
  kill $xv ;;
esac
code=$?
cp $cfg/logs/nightjar.log $r/ 2>/dev/null
cp $cfg/window.json $r/window.after.json 2>/dev/null
[ "${NIGHTJAR_TIMING_CSV:-}" != $r/timing.csv ] && cp $NIGHTJAR_TIMING_CSV $r/timing.csv 2>/dev/null
echo "$name exit $code"
