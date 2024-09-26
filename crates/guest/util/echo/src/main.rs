// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

#![no_main]

use risc0_zkvm::guest::env;

risc0_zkvm::guest::entry!(main);

pub fn main() {
    let message: Vec<u8> = env::read();
    env::commit_slice(message.as_slice());
}
