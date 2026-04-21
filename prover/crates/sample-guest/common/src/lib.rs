// Copyright 2026 Boundless Foundation, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use risc0_zkvm::sha::Digest;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
pub enum IterReq {
    Iter(u64),
    Composition(u64, Digest),
    CompositionKeccak([u8; 32], Digest, bool),
    DoubleComposition(u64, Digest, Digest),
    Keccak(String),
    KeccakUnion(usize),
    CompositionKeccakUnion(u64, Digest, usize),
}

pub const PANIC_STR: &str = "GUEST CODE PANIC TEST STRING";
