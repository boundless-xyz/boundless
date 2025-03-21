// Copyright (c) 2025 RISC Zero Inc,
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

//! Verify the receipt claim given as input and commit it.

#![no_main]

use risc0_zkvm::{guest::env, ReceiptClaim};

risc0_zkvm::guest::entry!(main);

pub fn main() {
    let claim: ReceiptClaim = env::read();
    env::verify_integrity(&claim).unwrap();
}
