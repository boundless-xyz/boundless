# Check if nvcc is available, otherwise install instructions
if ! command -v nvcc &> /dev/null; then
    echo "nvcc is missing, please run:"
    echo "sudo apt update"
    echo "sudo apt install nvidia-cuda-toolkit -y"
    echo "and try again"
    exit 1
fi

# Create a temporary CUDA file
cat << 'EOF' > /tmp/detect_cuda.cu
#include <cuda_runtime.h>
#include <stdio.h>
int main() {
    cudaDeviceProp prop;
    cudaGetDeviceProperties(&prop, 0);
    printf("%d.%d", prop.major, prop.minor);
    return 0;
}
EOF

# Compile and run the detection program
nvcc /tmp/detect_cuda.cu -o /tmp/detect_cuda
COMPUTE_CAP=$(/tmp/detect_cuda)

# Clean up temporary files
rm /tmp/detect_cuda.cu /tmp/detect_cuda

# Set the NVCC flags based on detected compute capability
export NVCC_APPEND_FLAGS="--gpu-architecture=compute_${COMPUTE_CAP} --gpu-code=compute_${COMPUTE_CAP},sm_${COMPUTE_CAP} --generate-code arch=compute_${COMPUTE_CAP},code=sm_${COMPUTE_CAP}"

echo $NVCC_APPEND_FLAGS
