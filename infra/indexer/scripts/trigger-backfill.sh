#!/bin/bash
set -e

usage() {
  cat << EOF
Usage: $0 [OPTIONS]

Trigger a market indexer backfill task via Lambda.

Required:
  --lambda-name NAME       Lambda function name
  --mode MODE             Backfill mode: 'statuses_and_aggregates' or 'aggregates'
  --start-block BLOCK     Starting block number

Optional:
  --end-block BLOCK       Ending block number
  --tx-fetch-strategy STR 'block-receipts' or 'tx-by-hash' (default: 'tx-by-hash')
  --region REGION         AWS region (default: 'us-west-2')

Example:
  $0 --lambda-name dev-backfill-trigger \\
     --mode statuses_and_aggregates \\
     --start-block 1000000 \\
     --end-block 2000000
EOF
  exit 1
}

# Defaults
REGION="us-west-2"
TX_FETCH_STRATEGY="tx-by-hash"

# Parse arguments
while [[ $# -gt 0 ]]; do
  case $1 in
    --lambda-name) LAMBDA_NAME="$2"; shift 2 ;;
    --mode) MODE="$2"; shift 2 ;;
    --start-block) START_BLOCK="$2"; shift 2 ;;
    --end-block) END_BLOCK="$2"; shift 2 ;;
    --tx-fetch-strategy) TX_FETCH_STRATEGY="$2"; shift 2 ;;
    --region) REGION="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "Unknown option: $1"; usage ;;
  esac
done

# Validate
if [ -z "$LAMBDA_NAME" ] || [ -z "$MODE" ] || [ -z "$START_BLOCK" ]; then
  echo "Error: Missing required arguments"
  usage
fi

# Build payload
PAYLOAD=$(cat <<EOF
{
  "mode": "$MODE",
  "startBlock": $START_BLOCK,
  "txFetchStrategy": "$TX_FETCH_STRATEGY"
EOF
)

if [ -n "$END_BLOCK" ]; then
  PAYLOAD="$PAYLOAD,
  \"endBlock\": $END_BLOCK"
fi

PAYLOAD="$PAYLOAD
}"

# Invoke Lambda
echo "Triggering backfill task..."
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

TASK_ARN=$(cat response.json | jq -r '.taskArn // empty')

if [ -n "$TASK_ARN" ]; then
  echo ""
  echo "✅ Task started: $TASK_ARN"
  
  # Extract cluster name and task ID from ARN
  # ARN format: arn:aws:ecs:region:account:task/cluster-name/task-id
  CLUSTER_NAME=$(echo "$TASK_ARN" | sed -n 's/.*task\/\([^/]*\)\/.*/\1/p')
  TASK_ID=$(echo "$TASK_ARN" | sed -n 's/.*task\/[^/]*\/\(.*\)/\1/p')
  
  LOG_GROUP=""
  
  if [ -n "$CLUSTER_NAME" ] && [ -n "$TASK_ID" ]; then
    # Wait a moment for task to be registered
    sleep 1
    
    # Try to get log group from task definition
    # First get the task definition ARN from the task
    TASK_DEF_ARN=$(aws ecs describe-tasks \
      --cluster "$CLUSTER_NAME" \
      --tasks "$TASK_ID" \
      --region "$REGION" \
      --query 'tasks[0].taskDefinitionArn' \
      --output text 2>/dev/null)
    
    if [ -n "$TASK_DEF_ARN" ] && [ "$TASK_DEF_ARN" != "None" ]; then
      # Get log group from task definition
      LOG_GROUP=$(aws ecs describe-task-definition \
        --task-definition "$TASK_DEF_ARN" \
        --region "$REGION" \
        --query 'taskDefinition.containerDefinitions[0].logConfiguration.options."awslogs-group"' \
        --output text 2>/dev/null)
    fi
  fi
  
  echo ""
  echo "Monitor logs:"
  if [ -n "$LOG_GROUP" ] && [ "$LOG_GROUP" != "None" ] && [ "$LOG_GROUP" != "" ]; then
    echo "  aws logs tail $LOG_GROUP --follow --region $REGION"
  else
    echo "  (Could not determine log group automatically - task may still be starting)"
    echo "  aws logs tail <log group> --follow --region $REGION"
  fi
else
  ERROR=$(cat response.json | jq -r '.error // empty')
  if [ -n "$ERROR" ]; then
    echo ""
    echo "❌ Error: $ERROR"
  fi
fi

rm response.json

