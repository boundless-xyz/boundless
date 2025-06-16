#!/bin/bash

# Get instance ID with tag Name=builder-local and in running state
INSTANCE_ID=$(aws ec2 describe-instances \
  --filters "Name=tag:Name,Values=builder-local" "Name=instance-state-name,Values=running" \
  --query "Reservations[].Instances[].InstanceId" \
  --output text)

if [ -z "$INSTANCE_ID" ]; then
  echo "No running EC2 instance found with tag Name=builder-local"
  exit 1
fi

echo "Stopping instance: $INSTANCE_ID"

aws ec2 stop-instances --instance-ids "$INSTANCE_ID" --output text