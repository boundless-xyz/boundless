#!/usr/bin/env bash
#
# self-host.sh — Self-host a guest program and input via Cloudflare Tunnel,
# then submit a Boundless proof request. No Pinata/S3 account needed.
#
# Usage:
#   self-host.sh <ELF_PATH> <INPUT_PATH> [OPTIONS]
#   self-host.sh <ELF_PATH> --input-hex <HEX> [OPTIONS]
#
# Examples:
#   # Serve ELF + input file, auto-submit and wait for fulfillment
#   self-host.sh ./my-guest.elf ./input.bin --submit --wait
#
#   # Just serve (print URLs, don't submit)
#   self-host.sh ./my-guest.elf ./input.bin
#
#   # With explicit image ID and pricing
#   self-host.sh ./my-guest.elf ./input.bin --image-id abc123... --submit --wait \
#       --min-price 0.0001 --max-price 0.003
#
set -euo pipefail

# ─── Colors ──────────────────────────────────────────────────────────────────
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
BOLD='\033[1m'
DIM='\033[2m'
NC='\033[0m'

# ─── Defaults ────────────────────────────────────────────────────────────────
PORT=""
DO_SUBMIT=false
DO_WAIT=false
IMAGE_ID=""
INPUT_HEX=""
MIN_PRICE="100000000000000"       # 0.0001 ETH
MAX_PRICE="2000000000000000"      # 0.002 ETH
RAMP_UP_PERIOD="300"              # 5 min
TIMEOUT="3600"                    # 1 hour
LOCK_TIMEOUT="2700"               # 45 min
LOCK_COLLATERAL="5000000000000000000"  # 5 ZKC
POLL_INTERVAL=15

# ─── Process tracking ───────────────────────────────────────────────────────
HTTP_PID=""
TUNNEL_PID=""
SERVE_DIR=""
SIGINT_COUNT=0

# ─── Parse arguments ────────────────────────────────────────────────────────
usage() {
    cat >&2 <<'EOF'
Usage: self-host.sh <ELF_PATH> [INPUT_PATH] [OPTIONS]

Arguments:
  ELF_PATH              Path to the guest program ELF binary
  INPUT_PATH            Path to the input file (optional if --input-hex used)

Options:
  --input-hex HEX       Provide input as hex string (alternative to INPUT_PATH)
  --image-id ID         Image ID (64-char hex). If omitted, you must provide it.
  --port PORT           Local HTTP server port (default: random)
  --submit              Build YAML and submit the request after tunnel is up
  --wait                Poll for fulfillment after submitting (implies --submit)
  --min-price WEI       Min price in wei (default: 100000000000000)
  --max-price WEI       Max price in wei (default: 2000000000000000)
  --timeout SECS        Request timeout in seconds (default: 3600)
  --poll-interval SECS  Status poll interval in seconds (default: 15)
  -h, --help            Show this help

Environment:
  REQUESTOR_RPC_URL          RPC endpoint (or configure via 'boundless requestor setup')
  REQUESTOR_PRIVATE_KEY      Private key (or configure via 'boundless requestor setup')
EOF
    exit 1
}

ELF_PATH=""
INPUT_PATH=""
POSITIONAL_INDEX=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --input-hex)   INPUT_HEX="$2"; shift 2 ;;
        --image-id)    IMAGE_ID="$2"; shift 2 ;;
        --port)        PORT="$2"; shift 2 ;;
        --submit)      DO_SUBMIT=true; shift ;;
        --wait)        DO_WAIT=true; DO_SUBMIT=true; shift ;;
        --min-price)   MIN_PRICE="$2"; shift 2 ;;
        --max-price)   MAX_PRICE="$2"; shift 2 ;;
        --timeout)     TIMEOUT="$2"; shift 2 ;;
        --poll-interval) POLL_INTERVAL="$2"; shift 2 ;;
        -h|--help)     usage ;;
        -*)            printf "${RED}Unknown option: %s${NC}\n" "$1" >&2; usage ;;
        *)
            if [[ $POSITIONAL_INDEX -eq 0 ]]; then
                ELF_PATH="$1"
            elif [[ $POSITIONAL_INDEX -eq 1 ]]; then
                INPUT_PATH="$1"
            else
                printf "${RED}Unexpected argument: %s${NC}\n" "$1" >&2; usage
            fi
            ((POSITIONAL_INDEX++))
            shift
            ;;
    esac
done

if [[ -z "$ELF_PATH" ]]; then
    printf "${RED}Error: ELF_PATH is required${NC}\n" >&2
    usage
fi

if [[ ! -f "$ELF_PATH" ]]; then
    printf "${RED}Error: ELF file not found: %s${NC}\n" "$ELF_PATH" >&2
    exit 1
fi

if [[ -z "$INPUT_PATH" && -z "$INPUT_HEX" ]]; then
    printf "${YELLOW}Warning: No input provided. Using empty input.${NC}\n" >&2
    INPUT_HEX="0x"
fi

if [[ -n "$INPUT_PATH" && ! -f "$INPUT_PATH" ]]; then
    printf "${RED}Error: Input file not found: %s${NC}\n" "$INPUT_PATH" >&2
    exit 1
fi

if [[ -z "$IMAGE_ID" ]]; then
    printf "${RED}Error: --image-id is required.${NC}\n" >&2
    printf "${DIM}Compute it from your ELF with the Boundless SDK or r0vm.${NC}\n" >&2
    exit 1
fi

# Strip 0x prefix from image ID if present
IMAGE_ID="${IMAGE_ID#0x}"

if ! echo "$IMAGE_ID" | grep -qE '^[0-9a-fA-F]{64}$'; then
    printf "${RED}Error: IMAGE_ID must be exactly 64 hex characters (got %d chars)${NC}\n" "${#IMAGE_ID}" >&2
    exit 1
fi

# ─── Check prerequisites ────────────────────────────────────────────────────
for cmd in cloudflared python3 curl; do
    if ! command -v "$cmd" &>/dev/null; then
        printf "${RED}Error: %s is required but not found.${NC}\n" "$cmd" >&2
        printf "Run: bash scripts/check-prerequisites.sh for install instructions.\n" >&2
        exit 1
    fi
done

if $DO_SUBMIT && ! command -v boundless &>/dev/null; then
    printf "${RED}Error: boundless CLI is required for --submit but not found.${NC}\n" >&2
    exit 1
fi

# ─── Cleanup ────────────────────────────────────────────────────────────────
cleanup() {
    local exit_code=${1:-$?}
    # Suppress errors during cleanup
    set +e
    [[ -n "$HTTP_PID" ]]   && kill "$HTTP_PID" 2>/dev/null && wait "$HTTP_PID" 2>/dev/null
    [[ -n "$TUNNEL_PID" ]] && kill "$TUNNEL_PID" 2>/dev/null && wait "$TUNNEL_PID" 2>/dev/null
    [[ -n "$SERVE_DIR" ]]  && rm -rf "$SERVE_DIR"
    exit $exit_code
}
trap cleanup EXIT

# ─── SIGINT handler ─────────────────────────────────────────────────────────
handle_sigint() {
    ((SIGINT_COUNT++)) || true
    local remaining=$((3 - SIGINT_COUNT))
    if [[ $SIGINT_COUNT -lt 3 ]]; then
        printf "\n"
        printf "${YELLOW}${BOLD}⚠️  Tunnel is still serving files to provers!${NC}\n"
        printf "${YELLOW}   Killing now may cause your request to fail and funds to be lost.${NC}\n"
        printf "${YELLOW}   Press Ctrl-C ${BOLD}${remaining}${NC}${YELLOW} more time(s) to force quit.${NC}\n"
        printf "\n"
    else
        printf "\n${RED}${BOLD}🛑 Force quitting. Cleaning up...${NC}\n"
        cleanup 130
    fi
}
trap handle_sigint INT

# ─── Prepare serving directory ───────────────────────────────────────────────
SERVE_DIR=$(mktemp -d)

# Copy ELF
ELF_BASENAME="program.elf"
cp "$ELF_PATH" "$SERVE_DIR/$ELF_BASENAME"
ELF_SIZE=$(wc -c < "$SERVE_DIR/$ELF_BASENAME" | tr -d ' ')

# Copy or create input file
INPUT_BASENAME="input.bin"
if [[ -n "$INPUT_PATH" ]]; then
    cp "$INPUT_PATH" "$SERVE_DIR/$INPUT_BASENAME"
elif [[ -n "$INPUT_HEX" ]]; then
    # Convert hex to binary file
    local_hex="${INPUT_HEX#0x}"
    if [[ -n "$local_hex" ]]; then
        echo "$local_hex" | xxd -r -p > "$SERVE_DIR/$INPUT_BASENAME"
    else
        touch "$SERVE_DIR/$INPUT_BASENAME"
    fi
fi
INPUT_SIZE=$(wc -c < "$SERVE_DIR/$INPUT_BASENAME" | tr -d ' ')

# ─── Pick a port ────────────────────────────────────────────────────────────
if [[ -z "$PORT" ]]; then
    PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("",0)); print(s.getsockname()[1]); s.close()')
fi

# ─── Start HTTP server ──────────────────────────────────────────────────────
printf "\n"
printf "${BOLD}🚀 Boundless Self-Hosted Request${NC}\n"
printf "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n"
printf "\n"
printf "${DIM}Starting local HTTP server on port %s...${NC}\n" "$PORT"

(cd "$SERVE_DIR" && python3 -m http.server "$PORT" --bind 127.0.0.1) &>/dev/null &
HTTP_PID=$!

# Give the server a moment to start
sleep 0.5
if ! kill -0 "$HTTP_PID" 2>/dev/null; then
    printf "${RED}Error: HTTP server failed to start on port %s${NC}\n" "$PORT" >&2
    exit 1
fi

# Verify local server is working
if ! curl -sf "http://127.0.0.1:${PORT}/${ELF_BASENAME}" -o /dev/null; then
    printf "${RED}Error: HTTP server not responding. Is port %s available?${NC}\n" "$PORT" >&2
    exit 1
fi

printf "${GREEN}  ✓ HTTP server running on port %s${NC}\n" "$PORT"

# ─── Start Cloudflare Tunnel ────────────────────────────────────────────────
printf "${DIM}Starting Cloudflare Tunnel...${NC}\n"

TUNNEL_LOG="$SERVE_DIR/tunnel.log"
cloudflared tunnel --url "http://127.0.0.1:${PORT}" &> "$TUNNEL_LOG" &
TUNNEL_PID=$!

# Wait for tunnel URL AND connection registration (up to 60 seconds)
# cloudflared logs the URL before the connection is actually registered,
# so we need to wait for both before attempting to verify.
TUNNEL_URL=""
TUNNEL_REGISTERED=false
for i in $(seq 1 120); do
    if [[ -f "$TUNNEL_LOG" ]]; then
        if [[ -z "$TUNNEL_URL" ]]; then
            TUNNEL_URL=$(grep -oE 'https://[a-zA-Z0-9-]+\.trycloudflare\.com' "$TUNNEL_LOG" | head -1 || true)
        fi
        if ! $TUNNEL_REGISTERED && grep -q "Registered tunnel connection" "$TUNNEL_LOG" 2>/dev/null; then
            TUNNEL_REGISTERED=true
        fi
        if [[ -n "$TUNNEL_URL" ]] && $TUNNEL_REGISTERED; then
            break
        fi
    fi
    sleep 0.5
done

if [[ -z "$TUNNEL_URL" ]]; then
    printf "${RED}Error: Cloudflare Tunnel failed to start within 60s.${NC}\n" >&2
    if grep -q "429" "$TUNNEL_LOG" 2>/dev/null; then
        printf "${YELLOW}Rate limited by Cloudflare (~20 quick tunnels/hour/IP).${NC}\n" >&2
        printf "${YELLOW}Wait 10-15 minutes and try again.${NC}\n" >&2
    fi
    printf "${DIM}Tunnel log:${NC}\n" >&2
    cat "$TUNNEL_LOG" >&2
    exit 1
fi

if ! $TUNNEL_REGISTERED; then
    printf "${YELLOW}Warning: Tunnel URL obtained but connection not confirmed registered.${NC}\n" >&2
    printf "${YELLOW}Proceeding anyway — verification may take longer.${NC}\n" >&2
fi

# Verify tunnel is accessible
PROGRAM_URL="${TUNNEL_URL}/${ELF_BASENAME}"
INPUT_URL="${TUNNEL_URL}/${INPUT_BASENAME}"

# Brief pause for DNS propagation after tunnel registration
sleep 3

printf "${DIM}Verifying tunnel accessibility...${NC}\n"

TUNNEL_OK=false
set +o pipefail  # pipefail interferes with curl's internal pipe handling
for i in $(seq 1 20); do
    HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" --max-time 10 "$PROGRAM_URL" 2>/dev/null || true)
    if [[ "$HTTP_CODE" == "200" ]]; then
        TUNNEL_OK=true
        break
    fi
    sleep 2
done
set -o pipefail

if ! $TUNNEL_OK; then
    printf "${RED}Error: Tunnel URL not reachable: %s${NC}\n" "$PROGRAM_URL" >&2
    printf "${DIM}Tunnel log (last 5 lines):${NC}\n" >&2
    tail -5 "$TUNNEL_LOG" >&2
    exit 1
fi

printf "${GREEN}  ✓ Tunnel is live${NC}\n"
printf "\n"

# ─── Display serving info ───────────────────────────────────────────────────
human_size() {
    local bytes=$1
    if [[ $bytes -ge 1048576 ]]; then
        printf "%.1f MB" "$(echo "scale=1; $bytes / 1048576" | bc)"
    elif [[ $bytes -ge 1024 ]]; then
        printf "%.1f KB" "$(echo "scale=1; $bytes / 1024" | bc)"
    else
        printf "%d B" "$bytes"
    fi
}

printf "  ${BOLD}📁 Program:${NC}   %s (%s)\n" "$(basename "$ELF_PATH")" "$(human_size "$ELF_SIZE")"
printf "  ${BOLD}📦 Input:${NC}     %s (%s)\n" "$(basename "${INPUT_PATH:-input.bin}")" "$(human_size "$INPUT_SIZE")"
printf "  ${BOLD}🔑 Image ID:${NC}  %s...%s\n" "${IMAGE_ID:0:16}" "${IMAGE_ID: -8}"
printf "  ${BOLD}🌐 Tunnel:${NC}    %s\n" "$TUNNEL_URL"
printf "\n"
printf "  ${CYAN}Program URL:${NC}  %s\n" "$PROGRAM_URL"
printf "  ${CYAN}Input URL:${NC}    %s\n" "$INPUT_URL"
printf "\n"
printf "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n"

# ─── Build YAML request file ────────────────────────────────────────────────
if $DO_SUBMIT; then
    RAMP_UP_START=$(( $(date +%s) + 30 ))
    YAML_PATH="$SERVE_DIR/request.yaml"

    # Hex-encode the input URL (the data field expects 0x + hex-encoded bytes)
    INPUT_URL_HEX="0x$(printf '%s' "$INPUT_URL" | xxd -p | tr -d '\n')"

    cat > "$YAML_PATH" <<YAML
# Boundless proof request — self-hosted via Cloudflare Tunnel
# Generated $(date -u +"%Y-%m-%d %H:%M:%S UTC")
# Tunnel: ${TUNNEL_URL}

id: 0
imageUrl: "${PROGRAM_URL}"
input:
  inputType: Url
  data: "${INPUT_URL_HEX}"
requirements:
  imageId: "${IMAGE_ID}"
  predicate:
    predicateType: PrefixMatch
    data: "0x${IMAGE_ID}"
  callback:
    addr: "0x0000000000000000000000000000000000000000"
    gasLimit: 0
  selector: "00000000"
offer:
  minPrice: ${MIN_PRICE}
  maxPrice: ${MAX_PRICE}
  rampUpStart: ${RAMP_UP_START}
  rampUpPeriod: ${RAMP_UP_PERIOD}
  timeout: ${TIMEOUT}
  lockTimeout: ${LOCK_TIMEOUT}
  lockCollateral: ${LOCK_COLLATERAL}
YAML

    printf "\n"
    printf "${BOLD}📋 Submitting proof request...${NC}\n"
    printf "${DIM}   YAML: %s${NC}\n" "$YAML_PATH"
    printf "\n"

    # Submit the request and capture output
    SUBMIT_OUTPUT=$(RUST_LOG=warn \
        boundless requestor submit-file "$YAML_PATH" --no-preflight 2>&1) || {
        printf "${RED}Error: Submission failed.${NC}\n" >&2
        printf "%s\n" "$SUBMIT_OUTPUT" >&2
        exit 1
    }

    printf "%s\n" "$SUBMIT_OUTPUT"

    # Extract request ID from output
    REQUEST_ID=$(echo "$SUBMIT_OUTPUT" | grep -oE '0x[0-9a-fA-F]+' | head -1)

    if [[ -z "$REQUEST_ID" ]]; then
        printf "${YELLOW}Warning: Could not parse request ID from output.${NC}\n" >&2
        printf "${DIM}You can check status manually once you have the ID.${NC}\n" >&2
    else
        printf "\n"
        printf "${GREEN}${BOLD}✓ Request submitted!${NC}\n"
        printf "  ${BOLD}Request ID:${NC} %s\n" "$REQUEST_ID"
        printf "  ${BOLD}Explorer:${NC}   https://explorer.boundless.network/orders/%s\n" "$REQUEST_ID"
        printf "\n"
    fi

    # ─── Poll for fulfillment ────────────────────────────────────────────
    if $DO_WAIT && [[ -n "$REQUEST_ID" ]]; then
        printf "${BOLD}⏳ Waiting for fulfillment...${NC}\n"
        printf "${YELLOW}${BOLD}⚠️  Keep this running! The tunnel must stay alive until the prover downloads your files.${NC}\n"
        printf "${DIM}   Polling every %ds. Ctrl-C x3 to force quit.${NC}\n" "$POLL_INTERVAL"
        printf "\n"

        LAST_STATUS=""
        while true; do
            STATUS_OUTPUT=$(RUST_LOG=warn \
                boundless requestor status "$REQUEST_ID" 2>&1) || true

            # Try to extract a status keyword
            CURRENT_STATUS=""
            if echo "$STATUS_OUTPUT" | grep -qi "fulfilled"; then
                CURRENT_STATUS="fulfilled"
            elif echo "$STATUS_OUTPUT" | grep -qi "locked"; then
                CURRENT_STATUS="locked"
            elif echo "$STATUS_OUTPUT" | grep -qi "expired"; then
                CURRENT_STATUS="expired"
            elif echo "$STATUS_OUTPUT" | grep -qi "submitted\|pending\|open"; then
                CURRENT_STATUS="submitted"
            fi

            if [[ "$CURRENT_STATUS" != "$LAST_STATUS" && -n "$CURRENT_STATUS" ]]; then
                case "$CURRENT_STATUS" in
                    submitted)
                        printf "  ${BLUE}⏳ Status: Submitted${NC} — waiting for a prover to lock...\n"
                        ;;
                    locked)
                        printf "  ${CYAN}🔒 Status: Locked${NC} — a prover is generating your proof!\n"
                        ;;
                    fulfilled)
                        printf "\n"
                        printf "${GREEN}${BOLD}🎉 Request fulfilled!${NC}\n"
                        printf "\n"
                        # Show final status details
                        printf "%s\n" "$STATUS_OUTPUT"
                        printf "\n"
                        printf "  ${BOLD}🔗 Explorer:${NC} https://explorer.boundless.network/orders/%s\n" "$REQUEST_ID"
                        printf "\n"
                        printf "${DIM}Retrieve your proof with:${NC}\n"
                        printf "  ${BOLD}boundless requestor get-proof %s${NC}\n" "$REQUEST_ID"
                        printf "\n"
                        exit 0
                        ;;
                    expired)
                        printf "\n"
                        printf "${RED}${BOLD}⏰ Request expired.${NC}\n"
                        printf "%s\n" "$STATUS_OUTPUT"
                        printf "\n"
                        printf "${DIM}The request timed out before a prover fulfilled it.${NC}\n"
                        printf "${DIM}This can happen if the price was too low or provers couldn't access the tunnel.${NC}\n"
                        exit 1
                        ;;
                esac
                LAST_STATUS="$CURRENT_STATUS"
            fi

            sleep "$POLL_INTERVAL"
        done
    fi

    # If not waiting, keep the tunnel alive
    if [[ -n "$REQUEST_ID" ]]; then
        printf "${YELLOW}${BOLD}⚠️  Keep this running!${NC}\n"
        printf "${YELLOW}   The tunnel must stay alive until the prover downloads your program and input.${NC}\n"
        printf "${YELLOW}   Once the request is locked, you can usually safely quit after ~30 seconds.${NC}\n"
        printf "${DIM}   Ctrl-C x3 to force quit.${NC}\n"
        printf "\n"
    fi
else
    # Not submitting — just serving
    printf "${BOLD}Serving files. Use the URLs above in your proof request.${NC}\n"
    printf "\n"
    printf "${DIM}Example YAML snippet:${NC}\n"
    EXAMPLE_HEX="0x$(printf '%s' "$INPUT_URL" | xxd -p | tr -d '\n')"
    printf "  imageUrl: \"${PROGRAM_URL}\"\n"
    printf "  input:\n"
    printf "    inputType: Url\n"
    printf "    data: \"${EXAMPLE_HEX}\"  # hex-encoded URL\n"
    printf "\n"
    printf "${DIM}Or submit directly:${NC}\n"
    printf "  self-host.sh %s %s --image-id %s --submit --wait\n" \
        "$ELF_PATH" "${INPUT_PATH:-"<input>"}" "$IMAGE_ID"
    printf "\n"
fi

# ─── Keep alive ─────────────────────────────────────────────────────────────
printf "${DIM}Tunnel running. Press Ctrl-C x3 to quit.${NC}\n"
while true; do
    # Check tunnel health
    if ! kill -0 "$TUNNEL_PID" 2>/dev/null; then
        printf "${RED}${BOLD}Error: Cloudflare Tunnel process died unexpectedly.${NC}\n" >&2
        exit 1
    fi
    if ! kill -0 "$HTTP_PID" 2>/dev/null; then
        printf "${RED}${BOLD}Error: HTTP server process died unexpectedly.${NC}\n" >&2
        exit 1
    fi
    sleep 5
done
