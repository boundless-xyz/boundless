# syntax=docker/dockerfile:1
# Bento bundle (GPU): combines agent (CUDA) and bento_cli into a single image.
# Built from intermediate images pushed by CI — no compilation, just COPY.

ARG GPU_AGENT_IMAGE
ARG BENTO_CLI_IMAGE

FROM ${BENTO_CLI_IMAGE} AS cli

# Use the GPU agent image as the base — it has CUDA runtime, RISC0, groth16, blake3 artifacts.
FROM ${GPU_AGENT_IMAGE}

COPY --from=cli /app/bento_cli /app/bento_cli

ENTRYPOINT ["/app/agent"]
