#!/bin/bash
set -euo pipefail

# Check if the SSH key is added to the SSH agent
if ! ssh-add -l | grep -q "id_ed25519_dev_docker_builder"; then
    echo "SSH key not found in SSH agent. Adding it..."
    ssh-add ~/.ssh/id_ed25519_dev_docker_builder
    if [ $? -ne 0 ]; then
        echo "Failed to add SSH key to agent. Please ensure the key exists and try again."
        exit 1
    fi
    echo "SSH key added successfully."
else
    echo "SSH key already present in SSH agent."
fi

INSTANCE_ID=$(aws ec2 describe-instances \
  --filters "Name=tag:Name,Values=builder-local" \
  --query "Reservations[*].Instances[*].InstanceId" \
  --output text)

if [ -n "$INSTANCE_ID" ]; then
    INSTANCE_STATE=$(aws ec2 describe-instances \
      --instance-ids $INSTANCE_ID \
      --query "Reservations[*].Instances[*].State.Name" \
      --output text)

    if [ "$INSTANCE_STATE" != "running" ]; then
        aws ec2 start-instances --instance-ids $INSTANCE_ID
        echo "Instance $INSTANCE_ID has been started."
    else
        echo "Instance $INSTANCE_ID is already running."
    fi
else
    echo "No instance found with the tag name 'builder-local'."
fi

# Step 1: Get the EC2 instance DNS name
echo "Fetching EC2 instance DNS..."
INSTANCE_DNS=$(aws ec2 describe-instances \
  --filters "Name=tag:Name,Values=builder-local" "Name=instance-state-name,Values=running" \
  --query 'Reservations[*].Instances[*].PublicDnsName' \
  --output text)

if [[ -z "$INSTANCE_DNS" ]]; then
  echo "No running instance with tag 'builder' found."
  exit 1
fi

echo "Found instance DNS: $INSTANCE_DNS"

# Step 2: Add the instance to known_hosts if not already present
if ! ssh-keygen -F "$INSTANCE_DNS" > /dev/null; then
  echo "Adding $INSTANCE_DNS to known_hosts..."
  if ! ssh-keyscan "$INSTANCE_DNS" 2>&1 | tee -a ~/.ssh/known_hosts; then
    echo "Error: Failed to add $INSTANCE_DNS to known_hosts. Please check your network connection and instance status."
    exit 1
  fi
  echo "Successfully added $INSTANCE_DNS to known_hosts"
else
  echo "$INSTANCE_DNS is already in known_hosts"
fi

# Step 3: Remove any previously registered builder, suppressing errors
# Typically there are errors as it tries to ssh into the previous instance, but at this point the instance may not exist.
echo "Removing any existing Docker builder named 'aws-builder' (if it exists). May take some time..."
docker buildx rm aws-builder > /dev/null 2>&1 || true

# Step 4: Register the new builder
echo "Creating new Docker builder..."
docker buildx create --name aws-builder --driver docker-container --platform linux/amd64 "ssh://ec2-user@$INSTANCE_DNS"

# Step 5: Set the environment variable
export DOCKER_REMOTE_BUILDER="aws-builder"
echo "DOCKER_REMOTE_BUILDER set to 'aws-builder'"

# Done
echo "Docker builder setup complete."