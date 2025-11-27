# syntax=docker/dockerfile:1.4
FROM rust:1.89.0-bookworm AS dependencies
WORKDIR /src/

# APT deps
RUN apt -qq update && \
  apt install -y -q build-essential cmake libgmp-dev libsodium-dev nasm curl m4 git protobuf-compiler clang

WORKDIR /src/

# Build rapidsnark
RUN git clone https://github.com/iden3/rapidsnark.git && \
  cd rapidsnark && \
  git checkout 808462cdcfdea6112269a4652d23999c8e4bc6d6

WORKDIR /src/rapidsnark/

RUN git submodule init && \
  git submodule update && \
  ./build_gmp.sh host && \
  make host && \
  cp ./package/bin/prover /usr/local/sbin/rapidsnark

# Build circom-witnesscalc
RUN git clone https://github.com/iden3/circom-witnesscalc.git && \
  cd circom-witnesscalc && \
  git checkout b7ff0ffd9c72c8f60896ce131ee98a35aba96009 && \
  cargo install --path .

WORKDIR /setup/
# Download prover files
ADD https://staging-signal-artifacts.beboundless.xyz/v3/proving/blake3_groth16_artifacts.tar.xz blake3_groth16_artifacts.tar.xz
RUN tar -xvf blake3_groth16_artifacts.tar.xz && \
  rm blake3_groth16_artifacts.tar.xz

# Create a final clean image with all the dependencies to perform stark->snark
FROM debian:bookworm-slim AS prover
RUN apt -qq update && \
  apt install -y -q build-essential cmake libgmp-dev libsodium-dev nasm curl m4

COPY --from=dependencies /usr/local/sbin/rapidsnark /usr/local/sbin/rapidsnark
COPY --from=dependencies /usr/local/cargo/bin/calc-witness /usr/local/sbin/calc-witness

WORKDIR /setup/
COPY --from=dependencies /setup/blake3_groth16_artifacts/verify_for_guest_graph.bin .
COPY --from=dependencies /setup/blake3_groth16_artifacts/verify_for_guest_final.zkey .

WORKDIR /app/
COPY scripts/prove.sh prove.sh
RUN chmod +x /app/prove.sh

ENTRYPOINT ["/app/prove.sh"]