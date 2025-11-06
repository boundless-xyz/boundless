# syntax=docker/dockerfile:1.4
FROM debian:bookworm-slim AS dependencies

WORKDIR /src/

# APT deps
RUN apt -qq update && \
  apt install -y -q build-essential cmake libgmp-dev libsodium-dev nasm curl m4 git

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


# Create a final clean image with all the dependencies to perform stark->snark
FROM debian:bookworm-slim AS prover

RUN apt -qq update && \
  apt install -y -q build-essential cmake libgmp-dev libsodium-dev nasm curl m4

COPY --from=dependencies /usr/local/sbin/rapidsnark /usr/local/sbin/rapidsnark

RUN ulimit -s unlimited

ENTRYPOINT ["/usr/local/sbin/rapidsnark"]