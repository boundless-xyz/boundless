# Check if nvcc is available, otherwise install instructions
if ! command -v nvcc &> /dev/null; then
    echo "nvcc is missing, please run:"
    echo "sudo apt update"
    echo "sudo apt install nvidia-cuda-toolkit -y"
    echo "and try again"
    exit 1
fi

# Instead of compiling just for one detected arch, support a range of common targets
ALL_ARCHS=("70" "75" "80" "86" "89")

# Create a temporary CUDA file
cat << 'EOF' > /tmp/detect_cuda.cu
#include <cuda_runtime.h>
#include <stdio.h>
int main() {
    cudaDeviceProp prop;
    cudaGetDeviceProperties(&prop, 0);
    printf("%d%d", prop.major, prop.minor);
    return 0;
}
EOF

nvcc /tmp/detect_cuda.cu -o /tmp/detect_cuda
DETECTED_ARCH=$(/tmp/detect_cuda)
rm /tmp/detect_cuda.cu /tmp/detect_cuda

# Only include architectures >= detected one
FLAGS=""
for arch in "${ALL_ARCHS[@]}"; do
  if (( arch >= DETECTED_ARCH )); then
    FLAGS+=" --generate-code=arch=compute_${arch},code=sm_${arch}"
  fi
done

# Use the *lowest* arch as --gpu-architecture baseline (required by NVCC)
LOWEST_ARCH=$(echo "${ALL_ARCHS[@]}" | tr ' ' '\n' | sort -n | head -n 1)
export NVCC_APPEND_FLAGS="--gpu-architecture=compute_${LOWEST_ARCH}${FLAGS}"

echo "$NVCC_APPEND_FLAGS"
