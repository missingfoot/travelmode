#!/usr/bin/env bash
# Phase 1 live verification. Run as root:  sudo ./dev/verify.sh
set -u
cd "$(dirname "$0")/.."

DAEMON=./target/debug/travelmoded
CLI="./target/debug/travelmode --socket /tmp/travelmode/daemon.sock"
CURL=$(command -v curl)
pass=0; fail=0

check() { # check <desc> <cmd...>
  local desc=$1; shift
  if "$@" >/dev/null 2>&1; then echo "PASS: $desc"; pass=$((pass+1));
  else echo "FAIL: $desc"; fail=$((fail+1)); fi
}

echo "== build =="
cargo build --workspace || exit 1

echo "== start daemon =="
pkill -9 -x travelmoded 2>/dev/null; sleep 0.5
rm -rf /tmp/travelmode
RUST_LOG=info $DAEMON --config dev/config.toml &
DPID=$!
sleep 1.5
kill -0 $DPID 2>/dev/null || { echo "daemon failed to start"; exit 1; }

echo "== baseline =="
check "status responds"            $CLI status
check "network responds"           $CLI network
check "ps responds"                $CLI ps
check "connections responds"       $CLI connections
check "filtering active in status" bash -c "$CLI --json status | grep -qE '\"filtering_active\": *true'"
check "nft table exists"           nft list table inet travelmode

echo "== allow-all baseline: curl works =="
check "curl succeeds before block" $CURL -s --max-time 5 -o /dev/null https://example.com

echo "== block curl =="
$CLI block "$CURL"
check "rule listed"                bash -c "$CLI rules | grep -qi curl"
sleep 0.2
$CURL -s --max-time 4 -o /dev/null https://example.com && { echo "FAIL: curl still works after block"; fail=$((fail+1)); } || { echo "PASS: curl blocked"; pass=$((pass+1)); }

echo "== pause =="
$CLI pause
check "curl works while paused"    $CURL -s --max-time 5 -o /dev/null https://example.com
$CLI resume
sleep 0.2
$CURL -s --max-time 4 -o /dev/null https://example.com && { echo "FAIL: curl works after resume (should be blocked)"; fail=$((fail+1)); } || { echo "PASS: blocked again after resume"; pass=$((pass+1)); }

echo "== unblock =="
RULE_ID=$($CLI --json rules | grep -oE '"id": *[0-9]+' | head -1 | grep -oE '[0-9]+')
$CLI remove "$RULE_ID"
sleep 0.2
check "curl works after remove"    $CURL -s --max-time 5 -o /dev/null https://example.com

echo "== persistence =="
$CLI block "$CURL"
kill -TERM $DPID; sleep 1
RUST_LOG=info $DAEMON --config dev/config.toml &
DPID=$!
sleep 1.5
check "rule survived restart"      bash -c "$CLI rules | grep -qi curl"
$CURL -s --max-time 4 -o /dev/null https://example.com && { echo "FAIL: curl works after daemon restart (should be blocked)"; fail=$((fail+1)); } || { echo "PASS: block enforced after restart"; pass=$((pass+1)); }

echo "== teardown =="
RULE_ID=$($CLI --json rules | grep -oE '"id": *[0-9]+' | head -1 | grep -oE '[0-9]+')
[ -n "$RULE_ID" ] && $CLI remove "$RULE_ID"
kill -TERM $DPID; sleep 1
check "nft table removed on exit"  bash -c "! nft list table inet travelmode 2>/dev/null"

echo
echo "RESULT: $pass passed, $fail failed"
[ $fail -eq 0 ]
