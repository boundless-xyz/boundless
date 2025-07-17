// Copyright 2025 RISC Zero, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
