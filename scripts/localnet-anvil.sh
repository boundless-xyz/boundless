#!/bin/bash
set -e

STATE_FILE="/data/state.json"

# Use --state (alias for --load-state + --dump-state) with periodic dumps
# --state-interval ensures state is saved every 60 seconds, not just on exit
exec anvil \
    -p 8545 \
    -b 2 \
    --host 0.0.0.0 \
    --state "$STATE_FILE" \
    --state-interval 60
