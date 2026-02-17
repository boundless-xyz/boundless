#!/bin/bash
set -e

usage() {
  cat << EOF
Usage: $0 [OPTIONS]

Trigger the indexer redrive Lambda to reset FAILED (and optionally stuck EXECUTING) cycle_counts
back to PENDING so the indexer picks them up again.

Required:
  --lambda-name NAME       Lambda function name

Optional:
  --lookback-days NUM      Only consider requests created in last N days (default: 3)
  --requestor ADDR        If set, only redrive requests for this client address (e.g. 0x...)
  --include-stuck         Also reset EXECUTING rows stuck > 1 hour
  --dry-run               Log what would be redriven without updating the DB
  --region REGION         AWS region (default: us-west-2)

Examples:
  Dry run (no DB changes):
    $0 --lambda-name l-prod-84532-indexer-redrive-lambda --lookback-days 3 --dry-run

  Redrive all failed in last 3 days:
    $0 --lambda-name l-prod-84532-indexer-redrive-lambda --lookback-days 3

  Redrive failed + stuck EXECUTING, last 7 days:
    $0 --lambda-name l-prod-84532-indexer-redrive-lambda --lookback-days 7 --include-stuck

  Redrive only for a specific requestor:
    $0 --lambda-name l-prod-84532-indexer-redrive-lambda --lookback-days 3 --requestor 0xABC...

Get lambda name:
  aws lambda list-functions --query 'Functions[?contains(FunctionName, \`redrive\`)].FunctionName' --output text
EOF
  exit 1
}

REGION="us-west-2"
LOOKBACK_DAYS="3"
REQUESTOR=""
INCLUDE_STUCK="false"
DRY_RUN="false"

while [[ $# -gt 0 ]]; do
  case $1 in
    --lambda-name) LAMBDA_NAME="$2"; shift 2 ;;
    --lookback-days) LOOKBACK_DAYS="$2"; shift 2 ;;
    --requestor) REQUESTOR="${2#0x}"; shift 2 ;;
    --include-stuck) INCLUDE_STUCK="true"; shift 1 ;;
    --dry-run) DRY_RUN="true"; shift 1 ;;
    --region) REGION="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "Unknown option: $1"; usage ;;
  esac
done

if [ -z "$LAMBDA_NAME" ]; then
  echo "Error: --lambda-name is required"
  usage
fi

PAYLOAD="{\"lookback_days\": $LOOKBACK_DAYS"
if [ "$INCLUDE_STUCK" = "true" ]; then
  PAYLOAD="$PAYLOAD, \"include_stuck_executing\": true"
fi
if [ "$DRY_RUN" = "true" ]; then
  PAYLOAD="$PAYLOAD, \"dry_run\": true"
fi
if [ -n "$REQUESTOR" ]; then
  PAYLOAD="$PAYLOAD, \"requestor\": \"$REQUESTOR\""
fi
PAYLOAD="$PAYLOAD}"

echo "Invoking redrive Lambda..."
echo "Lambda: $LAMBDA_NAME"
echo "Payload: $PAYLOAD"
echo ""

aws lambda invoke \
  --function-name "$LAMBDA_NAME" \
  --payload "$PAYLOAD" \
  --region "$REGION" \
  --cli-binary-format raw-in-base64-out \
  response.json

echo ""
echo "Response:"
cat response.json | jq .

REDRIVEN=$(cat response.json | jq -r '.redriven_count // empty')
MESSAGE=$(cat response.json | jq -r '.message // empty')

if [ -n "$REDRIVEN" ] || [ -n "$MESSAGE" ]; then
  echo ""
  if [ "$DRY_RUN" = "true" ]; then
    echo "Dry run completed. $MESSAGE"
  else
    echo "Redrove $REDRIVEN row(s). $MESSAGE"
  fi
  echo ""
  echo "View logs (request_id list in CloudWatch):"
  LOG_GROUP=$(aws lambda get-function-configuration --function-name "$LAMBDA_NAME" --region "$REGION" --query 'LoggingConfig.LogGroup' --output text 2>/dev/null || true)
  if [ -n "$LOG_GROUP" ] && [ "$LOG_GROUP" != "None" ]; then
    echo "  aws logs tail $LOG_GROUP --follow --region $REGION"
  else
    echo "  aws logs tail /aws/lambda/$LAMBDA_NAME --follow --region $REGION"
  fi
  echo ""
  echo "Filter recent logs:"
  if [ -n "$LOG_GROUP" ] && [ "$LOG_GROUP" != "None" ]; then
    echo "  aws logs filter-log-events --log-group-name $LOG_GROUP --region $REGION --limit 50"
  else
    echo "  aws logs filter-log-events --log-group-name /aws/lambda/$LAMBDA_NAME --region $REGION --limit 50"
  fi
else
  ERROR=$(cat response.json | jq -r '.errorMessage // .error // empty')
  if [ -n "$ERROR" ]; then
    echo ""
    echo "Error: $ERROR"
  fi
fi

rm -f response.json
