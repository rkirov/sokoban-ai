#!/usr/bin/env bash
# Run the solver over all Microban sets (and optionally more) and summarize.
set -u
BIN=./target/release/sokoban-solver
MODE="${1:-optimal}"
TIME_LIMIT="${2:-10}"
shift 2 2>/dev/null || true
FILES=("$@")
if [ ${#FILES[@]} -eq 0 ]; then
    FILES=(levels/microban1.txt levels/microban2.txt levels/microban3.txt levels/microban4.txt)
fi
overall_rc=0
for f in "${FILES[@]}"; do
    echo "=== $f (mode=$MODE, limit=${TIME_LIMIT}s) ==="
    "$BIN" "$f" --mode "$MODE" --time-limit "$TIME_LIMIT" --quiet
    rc=$?
    [ $rc -ne 0 ] && overall_rc=$rc
    echo
done
exit $overall_rc
