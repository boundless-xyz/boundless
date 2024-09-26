// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

//! Verify the receipt claim given as input and commit it.

#![no_main]

use risc0_zkvm::{guest::env, ReceiptClaim};

risc0_zkvm::guest::entry!(main);

pub fn main() {
    let claim: ReceiptClaim = env::read();
    env::verify_integrity(&claim).unwrap();
}
