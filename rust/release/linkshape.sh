#!/bin/sh
# Shape the receive direction (host to client) of a Linux client's network device, for the
# link-speed measurements in PERFORMANCE.md ("behaviour across client link speeds").
#
#   linkshape.sh up RATE_MBPS [QUEUE_MS [DELAY_MS [JITTER_MS [LOSS_PERCENT]]]]
#   linkshape.sh set RATE_MBPS [QUEUE_MS]      change the rate (and queue) of a running shaper
#   linkshape.sh down                          remove everything this script added
#   linkshape.sh status
#
# Incoming packets on $DEV are redirected to an ifb device whose root qdisc is
# [netem delay/jitter/loss] -> tbf at RATE_MBPS with a drop-tail queue of QUEUE_MS (default
# 50 ms) of that rate. Rates count IP bytes as the qdisc sees them, so the on-wire rate
# including Ethernet framing is about 4% higher; check the result with `iperf3 -u` from the
# host before trusting a rate (linkspeed.py does).
#
# $DEV defaults to eth0, the device inside the client's container, so the shaping is
# confined to that network namespace and leaves the machine's own network untouched. On a
# machine with root, DEV=eno1 shapes the real interface. Needs CAP_NET_ADMIN in the namespace.
set -eu
DEV=${DEV:-eth0}
IFB=${IFB:-ifb0}
TC=${TC:-$(command -v tc || echo /usr/sbin/tc)}

die() { echo "linkshape: $*" >&2; exit 1; }

# burst: two milliseconds of the rate, at least 6000 bytes; limit: QUEUE_MS of the rate
# on top of the burst, at least two full packets.
numbers() {
    rate_bps=$(awk -v r="$1" 'BEGIN { printf "%d", r * 1000000 }')
    queue_ms=${2:-50}
    burst=$(awk -v r="$rate_bps" 'BEGIN { b = r / 8 / 500; if (b < 6000) b = 6000; printf "%d", b }')
    limit=$(awk -v r="$rate_bps" -v q="$queue_ms" -v b="$burst" \
        'BEGIN { l = r / 8 * q / 1000 + b; if (l < 3200) l = 3200; printf "%d", l }')
}

tbf_args() {
    numbers "$1" "${2:-50}"
    echo "rate ${rate_bps}bit burst $burst limit $limit"
}

up() {
    [ $# -ge 1 ] || die "up needs RATE_MBPS"
    rate=$1; queue=${2:-50}; delay=${3:-0}; jitter=${4:-0}; loss=${5:-0}
    down quiet
    ip link add "$IFB" type ifb 2>/dev/null || ip link show "$IFB" >/dev/null 2>&1 || die "cannot create $IFB"
    ip link set "$IFB" up
    "$TC" qdisc add dev "$DEV" handle ffff: ingress
    "$TC" filter add dev "$DEV" parent ffff: protocol all matchall action mirred egress redirect dev "$IFB"
    if [ "$delay" != 0 ] || [ "$jitter" != 0 ] || [ "$loss" != 0 ]; then
        # A packet limit large enough that netem itself never drops: the tbf queue is the bottleneck.
        "$TC" qdisc add dev "$IFB" root handle 1: netem delay "${delay}ms" "${jitter}ms" loss "${loss}%" limit 100000
        # shellcheck disable=SC2046
        "$TC" qdisc add dev "$IFB" parent 1:1 handle 10: tbf $(tbf_args "$rate" "$queue")
    else
        # shellcheck disable=SC2046
        "$TC" qdisc add dev "$IFB" root handle 10: tbf $(tbf_args "$rate" "$queue")
    fi
    echo "linkshape: $DEV ingress -> $IFB at $rate Mbit/s, queue $queue ms, delay ${delay}ms +-${jitter}ms, loss ${loss}%"
}

set_rate() {
    [ $# -ge 1 ] || die "set needs RATE_MBPS"
    "$TC" qdisc show dev "$IFB" 2>/dev/null | grep -q tbf || die "not running; use up"
    handle=$("$TC" qdisc show dev "$IFB" | awk '/qdisc tbf/ { print $3 }' | head -1)
    parent=$("$TC" qdisc show dev "$IFB" | awk '/qdisc tbf/ { for (i = 1; i <= NF; i++) if ($i == "parent") print $(i + 1) }' | head -1)
    # shellcheck disable=SC2046
    if [ -n "$parent" ]; then
        "$TC" qdisc change dev "$IFB" parent "$parent" handle "$handle" tbf $(tbf_args "$1" "${2:-50}")
    else
        "$TC" qdisc change dev "$IFB" root handle "$handle" tbf $(tbf_args "$1" "${2:-50}")
    fi
    echo "linkshape: rate $1 Mbit/s, queue ${2:-50} ms"
}

down() {
    "$TC" qdisc del dev "$DEV" ingress 2>/dev/null || true
    ip link del "$IFB" 2>/dev/null || true
    [ "${1:-}" = quiet ] || echo "linkshape: down"
}

status() {
    "$TC" -s qdisc show dev "$DEV"
    "$TC" -s qdisc show dev "$IFB" 2>/dev/null || echo "$IFB: absent"
}

cmd=${1:-}; [ $# -gt 0 ] && shift
case $cmd in
    up) up "$@" ;;
    set) set_rate "$@" ;;
    down) down ;;
    status) status ;;
    *) sed -n '2,22p' "$0"; exit 2 ;;
esac
