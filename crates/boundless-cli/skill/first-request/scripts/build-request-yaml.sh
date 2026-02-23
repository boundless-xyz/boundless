#!/usr/bin/env bash
# Build a Boundless proof request YAML file from discovery script output.
#
# Usage:
#   bash build-request-yaml.sh <IMAGE_URL> <IMAGE_ID> <INPUT_DATA_HEX>
#
# Example:
#   bash build-request-yaml.sh \
#     "https://gateway.beboundless.cloud/ipfs/bafkrei..." \
#     "34a5c9394fb2fd3298ece07c16ec2ed009f6029a360f90f4e93933b55e2184d4" \
#     "0x0181a5737464696edc001000005001000000001a2bcc86cc81ccaacccdccc55e"
#
# Outputs YAML to stdout. Redirect to a file:
#   bash build-request-yaml.sh <URL> <ID> <HEX> > /tmp/boundless-request.yaml

set -euo pipefail

if [ $# -ne 3 ]; then
  echo "Usage: $0 <IMAGE_URL> <IMAGE_ID> <INPUT_DATA_HEX>" >&2
  echo "" >&2
  echo "  IMAGE_URL       IPFS/HTTP URL to the guest program ELF" >&2
  echo "  IMAGE_ID        64-char hex image ID (no 0x prefix)" >&2
  echo "  INPUT_DATA_HEX  Hex-encoded input data (with 0x prefix)" >&2
  exit 1
fi

IMAGE_URL="$1"
IMAGE_ID="$2"
INPUT_DATA="$3"

# Strip 0x prefix from image ID if accidentally included
IMAGE_ID="${IMAGE_ID#0x}"

# Ensure input data has 0x prefix
if [[ "$INPUT_DATA" != 0x* ]]; then
  INPUT_DATA="0x${INPUT_DATA}"
fi

# Validate image ID is 64 hex chars
if ! echo "$IMAGE_ID" | grep -qE '^[0-9a-fA-F]{64}$'; then
  echo "Error: IMAGE_ID must be exactly 64 hex characters (got ${#IMAGE_ID})" >&2
  exit 1
fi

# rampUpStart: 30 seconds from now
RAMP_UP_START=$(( $(date +%s) + 30 ))

cat <<EOF
# Boundless proof request — generated $(date -u +"%Y-%m-%d %H:%M:%S UTC")
# Submit with: boundless requestor submit-file <this-file> --no-preflight

id: 0
imageUrl: "${IMAGE_URL}"
input:
  inputType: Inline
  data: "${INPUT_DATA}"
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
  minPrice: 100000000000000
  maxPrice: 2000000000000000
  rampUpStart: ${RAMP_UP_START}
  rampUpPeriod: 300
  timeout: 3600
  lockTimeout: 2700
  lockCollateral: 5000000000000000000
EOF
