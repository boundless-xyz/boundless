#!/bin/bash
set -e -u -o pipefail

export CC="sccache clang"
export CXX="sccache clang++"
export SCCACHE_IDLE_TIMEOUT=0
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0
export CMAKE_C_COMPILER_LAUNCHER=sccache
export CMAKE_CXX_COMPILER_LAUNCHER=sccache
export RUSTC_WRAPPER=sccache

CREDS_FILE_SIZE=$(stat -c%s /root/.aws/credentials)
if [ $CREDS_FILE_SIZE -gt 0 ]; then
    export SCCACHE_BUCKET="risc0-ci-cache"
    export SCCACHE_REGION="us-west-2"
    export SCCACHE_S3_KEY_PREFIX="shared/boundless/rust-cache-docker-Linux-X64/sccache"

    echo "Using s3 [$SCCACHE_BUCKET] caching and sccache..."
    # TODO: param these correctly and detect OS / arch
else
    echo "Using local sccache"

    export SCCACHE_DIR=/root/.cache/sccache/
fi