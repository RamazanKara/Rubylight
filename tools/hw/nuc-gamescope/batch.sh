#!/bin/bash
# batch.sh <flatpak bundle>: the nightjar-gamescope-nuc runs, one after another, in the
# nj-gamescope container (container.sh setup and build first; the isolated host streams):
#   s-xvfb    60 s autostream outside gamescope (Xvfb), the timings' baseline
#   s-gs      the same under headless gamescope 1280x800@60
#   s-gshdr   gamescope --hdr-enabled (--hdr-debug-force-support: headless has no HDR display), an HDR stream
#   l-gs      the launcher under gamescope driven by pad.py alone: A opens the PC, A starts Desktop
#   f-gs      the release's Flatpak under gamescope, SDR
#   f-gshdr   the Flatpak under gamescope with HDR, gamescope's Vulkan layer installed
#   f-nosock  the same without the gamescope socket (--nofilesystem): the layer goes off, SDR with the note
# Prints each run's output decision, the timings (timings.py) and the checks; exit 1 if one fails.
set -u
W=${W:-$HOME/deck} here=$(cd "$(dirname "$0")" && pwd) bundle=$1
inside() {
  local envs=()
  while [[ $1 == *=* ]]; do envs+=(-e "$1"); shift; done
  docker exec -u $(id -u) -e HOME=$HOME -e W=$W "${envs[@]}" nj-gamescope "$@"
}
mkdir -p $W/runs
force=GSX=--hdr-debug-force-support
inside bash $here/run.sh s-xvfb xvfb 0 auto
inside bash $here/run.sh s-gs gs 0 auto
inside $force bash $here/run.sh s-gshdr gshdr 1 auto
# The pad first, so SDL finds it when Nightjar starts (its node is opened up 1 s after it appears).
docker exec nj-gamescope python $here/pad.py >$W/runs/pad.log 2>&1 &
for i in $(seq 20); do grep -q '^pad ' $W/runs/pad.log 2>/dev/null && break; sleep 0.5; done
inside T=100 bash $here/run.sh l-gs gs 0 launcher
wait
inside bash $here/flatpak.sh $bundle || { echo "FAIL flatpak install"; exit 1; }
inside bash $here/run.sh f-gs fp 0 auto
inside $force bash $here/run.sh f-gshdr fphdr 1 auto
inside $force FPARGS=--nofilesystem=xdg-run/gamescope-0 bash $here/run.sh f-nosock fphdr 1 auto

cd $W/runs
fail=0
check() { # check <run> <what> <regex>
  if grep -qE "$3" $1/stderr.log $1/nightjar.log 2>/dev/null; then echo "ok   $1: $2"; else echo "FAIL $1: $2"; fail=1; fi
}
for r in s-xvfb s-gs s-gshdr l-gs f-gs f-gshdr f-nosock; do
  echo "== $r"
  grep -hE 'nightjar: (renderer|gamescope|HDR asked|streaming |stream health|stream ended)' $r/stderr.log | sort -u | cut -c1-240
done
echo "== pad"; cat pad.log
echo "== timings (p50/p95 ms after the first 5 s)"
python3 $here/timings.py s-xvfb/timing.csv s-gs/timing.csv s-gshdr/timing.csv l-gs/timing.csv f-gs/timing.csv f-gshdr/timing.csv f-nosock/timing.csv 2>&1
echo "== checks"
for r in s-gs s-gshdr l-gs f-gs f-gshdr; do check $r "starts fullscreen without a border" 'gamescope: starting fullscreen'; done
for r in s-xvfb s-gs s-gshdr l-gs f-gs f-gshdr f-nosock; do check $r "streams" 'nightjar: streaming '; done
check s-gshdr "HDR output under gamescope" 'output HDR as'
check f-gshdr "HDR output from the Flatpak under gamescope" 'output HDR as'
check f-nosock "the layer goes off without its socket" 'Vulkan layer is off'
check f-nosock "SDR without the socket, with the note" 'HDR asked for, streaming in SDR'
check l-gs "the pad started the stream" 'nightjar: streaming '
for r in s-gs s-gshdr l-gs; do
  if cmp -s $r/window.before.json $r/window.after.json; then echo "ok   $r: the saved window place stays"; else echo "FAIL $r: window.json changed"; fail=1; fi
done
exit $fail
