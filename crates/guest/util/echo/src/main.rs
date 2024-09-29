// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use risc0_zkvm::guest::env;

pub fn main() {
    let message: Vec<u8> = env::read();
    env::commit_slice(message.as_slice());
}
