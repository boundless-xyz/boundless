// Copyright (c) 2025 RISC Zero Inc,
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use std::io::Read;

use risc0_zkvm::guest::env;

pub fn main() {
    // Read the entire input stream as raw bytes.
    let mut message = Vec::<u8>::new();
    env::stdin().read_to_end(&mut message).unwrap();

    // Commit exactly what the host provided to the journal.
    env::commit_slice(message.as_slice());
}
