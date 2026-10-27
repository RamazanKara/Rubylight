#!/bin/bash
# One streaming leg of scripts/pyrowave-linux.ps1 on the Linux client machine: runs nightjar
# headless (SDL's offscreen video driver, the "gpu" renderer on the machine's Vulkan device) in a
# container with the machine's /dev/dri, streams from <host:port> for <seconds> and leaves the
# timing CSV and the client's stderr in <outdir>.
#
#   pyrowave-linux-leg.sh <bindir> <cfgdir> <outdir> <name> <seconds> <host:port> <pin> [image]
#
# <bindir> holds nightjar and, for the GPU decoder, libpyrowave-shared.so (the package folder of
# package-linux.sh); <cfgdir> holds settings.json and, once paired, hosts.json.
set -u
bindir=$(cd "$1" && pwd) cfgdir=$(cd "$2" && pwd) outdir=$(mkdir -p "$3" && cd "$3" && pwd)
name=$4 seconds=$5 address=$6 pin=$7 image=${8:-nj-g3-build:1}
timeout $((seconds + 90)) docker run --rm --device /dev/dri \
	-e SDL_VIDEO_DRIVER=offscreen -e NIGHTJAR_CONFIG_DIR=/cfg -e NIGHTJAR_TIMING_CSV=/out/$name.csv \
	-e NIGHTJAR_EARLY_DECODE=0 -e NIGHTJAR_AUTOSTREAM="$address" -e NIGHTJAR_AUTOSTREAM_APP=Desktop \
	-e NIGHTJAR_AUTOSTREAM_PIN="$pin" -e NIGHTJAR_AUTOSTREAM_SECONDS="$seconds" \
	-v "$cfgdir":/cfg -v "$outdir":/out -v "$bindir":/app:ro "$image" /app/nightjar \
	>"$outdir/$name.stdout.log" 2>"$outdir/$name.log"
code=$?
echo "$name exit $code"
exit $code
