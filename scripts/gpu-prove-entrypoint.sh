#!/bin/bash
# Entrypoint script for GPU prove agent that automatically detects and uses all available GPUs
# Each GPU gets its own agent process

set -e

# Check if nvidia-smi is available
if ! command -v nvidia-smi &> /dev/null; then
    echo "ERROR: nvidia-smi not found. Cannot detect GPUs." >&2
    exit 1
fi

# Get list of GPU device IDs
GPU_IDS=$(nvidia-smi --list-gpus | awk '{print $2}' | sed 's/://' | tr '\n' ' ')

if [ -z "$GPU_IDS" ]; then
    echo "ERROR: No GPUs detected." >&2
    exit 1
fi

# Convert space-separated to array
read -ra GPU_ARRAY <<< "$GPU_IDS"

GPU_COUNT=${#GPU_ARRAY[@]}
echo "Detected $GPU_COUNT GPU(s): ${GPU_ARRAY[*]}"

# Array to store PIDs
PIDS=()

# Function to cleanup on exit
cleanup() {
    echo "Received signal, stopping all GPU agents..."
    for pid in "${PIDS[@]}"; do
        if kill -0 "$pid" 2>/dev/null; then
            echo "Stopping agent PID $pid"
            kill "$pid" 2>/dev/null || true
        fi
    done
    # Wait for processes to terminate
    wait
    exit 0
}

# Set up signal handlers
trap cleanup SIGTERM SIGINT

# Function to start agent for a specific GPU
start_agent_for_gpu() {
    local gpu_id=$1
    echo "Starting prove agent for GPU $gpu_id"
    CUDA_VISIBLE_DEVICES=$gpu_id /app/agent -t prove --redis-ttl ${REDIS_TTL:-57600} &
    local pid=$!
    PIDS+=($pid)
    echo "GPU $gpu_id agent started with PID $pid"
}

# Start an agent process for each GPU
for gpu_id in "${GPU_ARRAY[@]}"; do
    start_agent_for_gpu "$gpu_id"
done

echo "Started ${#PIDS[@]} GPU prove agent(s). Waiting for processes..."

# Wait for all processes (will be interrupted by signal handler)
wait
