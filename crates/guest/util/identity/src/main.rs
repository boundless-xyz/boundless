// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

//! Verify the receipt given as input and commit to its claim digest.

#![no_main]

use risc0_zkvm::{
    guest::env,
    sha::{Digest, Digestible},
    Receipt,
};

risc0_zkvm::guest::entry!(main);

pub fn main() {
    let bytes = env::read_frame();
    let (image_id, receipt): (Digest, Receipt) =
        postcard::from_bytes(&bytes).expect("failed to deserialize input");

    let claim = receipt.claim().unwrap();
    receipt.verify(image_id).unwrap();

    env::commit_slice(claim.digest().as_bytes());
}
