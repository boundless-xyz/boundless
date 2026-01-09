#!/bin/bash
# Entrypoint script for GPU prove agent that automatically detects and uses all available GPUs
# Each GPU gets its own agent process

set -e

# Check if nvidia-smi is available
if ! command -v nvidia-smi &> /dev/null; then
    echo "ERROR: nvidia-smi not found. Cannot detect GPUs." >&2
    exit 1
fi

# Get list of GPU device IDs using query format (more reliable)
GPU_IDS=$(nvidia-smi --query-gpu=index --format=csv,noheader,nounits | tr '\n' ' ')

if [ -z "$GPU_IDS" ]; then
    echo "ERROR: No GPUs detected." >&2
    exit 1
fi

# Convert space-separated to array
read -ra GPU_ARRAY <<< "$GPU_IDS"

GPU_COUNT=${#GPU_ARRAY[@]}
echo "Detected $GPU_COUNT GPU(s): ${GPU_ARRAY[*]}"

# Array to store PIDs
declare -A PIDS  # Associative array: GPU_ID -> PID

# Track if we're shutting down
SHUTTING_DOWN=false

# Function to cleanup on exit
cleanup() {
    SHUTTING_DOWN=true
    echo "Received signal, stopping all GPU agents..."
    for gpu_id in "${!PIDS[@]}"; do
        local pid=${PIDS[$gpu_id]}
        if kill -0 "$pid" 2>/dev/null; then
            echo "Stopping GPU $gpu_id agent (PID $pid)"
            kill -TERM "$pid" 2>/dev/null || true
        fi
    done
    # Wait with timeout
    local timeout=30
    local elapsed=0
    while [ $elapsed -lt $timeout ]; do
        local running=0
        for pid in "${PIDS[@]}"; do
            if kill -0 "$pid" 2>/dev/null; then
                ((running++))
            fi
        done
        [ $running -eq 0 ] && break
        sleep 1
        ((elapsed++))
    done
    # Force kill any remaining
    for pid in "${PIDS[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            echo "Force killing PID $pid"
            kill -9 "$pid" 2>/dev/null || true
        fi
    done
    exit 0
}

# Set up signal handlers
trap cleanup SIGTERM SIGINT

# Function to start agent for a specific GPU
start_agent_for_gpu() {
    local gpu_id=$1
    echo "Starting prove agent for GPU $gpu_id"
    CUDA_VISIBLE_DEVICES=$gpu_id /app/agent -t prove --redis-ttl "${REDIS_TTL:-57600}" &
    local pid=$!
    PIDS[$gpu_id]=$pid
    echo "GPU $gpu_id agent started with PID $pid"
}

# Start an agent process for each GPU
for gpu_id in "${GPU_ARRAY[@]}"; do
    start_agent_for_gpu "$gpu_id"
done

echo "Started ${#PIDS[@]} GPU prove agent(s). Monitoring processes..."

# Monitor processes and restart if they fail
while true; do
    for gpu_id in "${!PIDS[@]}"; do
        pid=${PIDS[$gpu_id]}
        if ! kill -0 "$pid" 2>/dev/null; then
            if [ "$SHUTTING_DOWN" = true ]; then
                continue
            fi
            wait "$pid" 2>/dev/null || true
            exit_code=$?
            echo "WARNING: GPU $gpu_id agent (PID $pid) exited with code $exit_code"
            echo "Restarting agent for GPU $gpu_id..."
            start_agent_for_gpu "$gpu_id"
        fi
    done
    sleep 5
done
