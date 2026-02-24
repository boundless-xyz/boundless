#!/bin/bash
set -e

# Defaults
INTERVAL=60
COUNT=10
SUBMIT_OFFCHAIN=""
INPUT_MAX_MCYCLES=1000
MIN="0.001"
MAX="0.002"
RAMP_UP=240
LOCK_TIMEOUT=900
TIMEOUT=1800
SECONDS_PER_MCYCLE=20
RAMP_UP_SECONDS_PER_MCYCLE=20
EXEC_RATE_KHZ=2000
LOCK_COLLATERAL_RAW="0"
TX_TIMEOUT=180
REGION="us-west-2"
ACTION="start"

usage() {
  cat << EOF
Usage: $0 [OPTIONS]

Trigger or cancel a load test via Lambda.

Required:
  --lambda-name NAME          Lambda function name

Actions:
  --cancel                    Cancel a running load test (default: start)
  --task-arn ARN              Task ARN to cancel (required with --cancel)

Load test parameters (for start action):
  --interval SECONDS          Seconds between requests (default: ${INTERVAL})
  --count N                   Number of requests to submit (default: ${COUNT})
  --submit-offchain           Submit requests offchain (default: onchain)
  --input-max-mcycles N       Max cycle count in Mcycles for random generation (default: ${INPUT_MAX_MCYCLES})
  --min PRICE                 Minimum price per mcycle in ether (default: ${MIN})
  --max PRICE                 Maximum price per mcycle in ether (default: ${MAX})
  --ramp-up SECONDS           Ramp-up period in seconds (default: ${RAMP_UP})
  --lock-timeout SECONDS      Lock-in timeout in seconds (default: ${LOCK_TIMEOUT})
  --timeout SECONDS           Request expiry timeout in seconds (default: ${TIMEOUT})
  --seconds-per-mcycle N      Additional seconds per 1M cycles added to timeout (default: ${SECONDS_PER_MCYCLE})
  --ramp-up-seconds-per-mcycle N  Additional seconds per 1M cycles added to ramp-up (default: ${RAMP_UP_SECONDS_PER_MCYCLE})
  --exec-rate-khz N           Execution rate in kHz for bidding start delay (default: ${EXEC_RATE_KHZ})
  --lock-collateral-raw WEI   Lock-in stake amount in ether (default: ${LOCK_COLLATERAL_RAW})
  --tx-timeout SECONDS        Transaction timeout in seconds (default: ${TX_TIMEOUT})
  --region REGION             AWS region (default: ${REGION})

Examples:
  Start a load test:
    $0 --lambda-name dev-og-load-test-84532-trigger \\
       --interval 10 --count 100 --submit-offchain

  Cancel a running load test:
    $0 --lambda-name dev-og-load-test-84532-trigger \\
       --cancel --task-arn arn:aws:ecs:us-west-2:123456:task/cluster/task-id
EOF
  exit 1
}

# Parse arguments
while [[ $# -gt 0 ]]; do
  case $1 in
    --lambda-name) LAMBDA_NAME="$2"; shift 2 ;;
    --cancel) ACTION="cancel"; shift ;;
    --task-arn) TASK_ARN="$2"; shift 2 ;;
    --interval) INTERVAL="$2"; shift 2 ;;
    --count) COUNT="$2"; shift 2 ;;
    --submit-offchain) SUBMIT_OFFCHAIN="true"; shift ;;
    --input-max-mcycles) INPUT_MAX_MCYCLES="$2"; shift 2 ;;
    --min) MIN="$2"; shift 2 ;;
    --max) MAX="$2"; shift 2 ;;
    --ramp-up) RAMP_UP="$2"; shift 2 ;;
    --lock-timeout) LOCK_TIMEOUT="$2"; shift 2 ;;
    --timeout) TIMEOUT="$2"; shift 2 ;;
    --seconds-per-mcycle) SECONDS_PER_MCYCLE="$2"; shift 2 ;;
    --ramp-up-seconds-per-mcycle) RAMP_UP_SECONDS_PER_MCYCLE="$2"; shift 2 ;;
    --exec-rate-khz) EXEC_RATE_KHZ="$2"; shift 2 ;;
    --lock-collateral-raw) LOCK_COLLATERAL_RAW="$2"; shift 2 ;;
    --tx-timeout) TX_TIMEOUT="$2"; shift 2 ;;
    --region) REGION="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "Unknown option: $1"; usage ;;
  esac
done

# Validate
if [ -z "$LAMBDA_NAME" ]; then
  echo "Error: --lambda-name is required"
  usage
fi

if [ "$ACTION" = "cancel" ]; then
  if [ -z "$TASK_ARN" ]; then
    echo "Error: --task-arn is required for cancel action"
    usage
  fi

  PAYLOAD=$(cat <<EOF
{
  "action": "cancel",
  "taskArn": "$TASK_ARN"
}
EOF
)
else
  SUBMIT_OFFCHAIN_JSON="false"
  if [ "$SUBMIT_OFFCHAIN" = "true" ]; then
    SUBMIT_OFFCHAIN_JSON="true"
  fi

  PAYLOAD=$(cat <<EOF
{
  "action": "start",
  "interval": $INTERVAL,
  "count": $COUNT,
  "submitOffchain": $SUBMIT_OFFCHAIN_JSON,
  "inputMaxMCycles": $INPUT_MAX_MCYCLES,
  "min": "$MIN",
  "max": "$MAX",
  "rampUp": $RAMP_UP,
  "lockTimeout": $LOCK_TIMEOUT,
  "timeout": $TIMEOUT,
  "secondsPerMCycle": $SECONDS_PER_MCYCLE,
  "rampUpSecondsPerMCycle": $RAMP_UP_SECONDS_PER_MCYCLE,
  "execRateKhz": $EXEC_RATE_KHZ,
  "lockCollateralRaw": "$LOCK_COLLATERAL_RAW",
  "txTimeout": $TX_TIMEOUT
}
EOF
)
fi

echo "Triggering load test ($ACTION)..."
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

if [ "$ACTION" = "start" ]; then
  TASK_ARN=$(cat response.json | jq -r '.taskArn // empty')

  if [ -n "$TASK_ARN" ]; then
    echo ""
    echo "Task started: $TASK_ARN"

    CLUSTER_NAME=$(echo "$TASK_ARN" | sed -n 's/.*task\/\([^/]*\)\/.*/\1/p')
    TASK_ID=$(echo "$TASK_ARN" | sed -n 's/.*task\/[^/]*\/\(.*\)/\1/p')

    LOG_GROUP=""

    if [ -n "$CLUSTER_NAME" ] && [ -n "$TASK_ID" ]; then
      sleep 1

      TASK_DEF_ARN=$(aws ecs describe-tasks \
        --cluster "$CLUSTER_NAME" \
        --tasks "$TASK_ID" \
        --region "$REGION" \
        --query 'tasks[0].taskDefinitionArn' \
        --output text 2>/dev/null)

      if [ -n "$TASK_DEF_ARN" ] && [ "$TASK_DEF_ARN" != "None" ]; then
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
      echo "  aws logs tail <log-group> --follow --region $REGION"
    fi

    echo ""
    echo "Cancel this load test:"
    echo "  $0 --lambda-name $LAMBDA_NAME --cancel --task-arn $TASK_ARN"
  else
    ERROR=$(cat response.json | jq -r '.error // empty')
    if [ -n "$ERROR" ]; then
      echo ""
      echo "Error: $ERROR"
    fi
  fi
else
  echo ""
  RESULT_TASK_ARN=$(cat response.json | jq -r '.taskArn // empty')
  if [ -n "$RESULT_TASK_ARN" ]; then
    echo "Task cancelled: $RESULT_TASK_ARN"
  fi
fi

rm -f response.json
