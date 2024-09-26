#!/bin/bash
set -e -u -o pipefail

echo "Using local sccache"
export CC="sccache clang"
export CXX="sccache clang++"
export CMAKE_C_COMPILER_LAUNCHER=sccache
export CMAKE_CXX_COMPILER_LAUNCHER=sccache
export RUSTC_WRAPPER=sccache
export SCCACHE_IDLE_TIMEOUT=0
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=0
export SCCACHE_DIR=/root/.cache/sccache/
