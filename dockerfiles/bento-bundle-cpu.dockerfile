# syntax=docker/dockerfile:1
# Bento bundle (CPU): combines agent (no CUDA) and bento_cli into a single image.
# Built from intermediate images pushed by CI — no compilation, just COPY.

ARG CPU_AGENT_IMAGE
ARG BENTO_CLI_IMAGE

FROM ${BENTO_CLI_IMAGE} AS cli

# Use the CPU agent image as the base.
FROM ${CPU_AGENT_IMAGE}

COPY --from=cli /app/bento_cli /app/bento_cli

ENTRYPOINT ["/app/agent"]
