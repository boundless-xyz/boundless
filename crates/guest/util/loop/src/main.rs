// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use risc0_zkvm::guest::env;

pub fn main() {
    let cycles: u64 = env::read();
    let mut last_cycles = env::cycle_count();
    let mut tot_cycles = last_cycles;

    while tot_cycles < cycles {
        let now_cycles = env::cycle_count();
        if now_cycles <= last_cycles {
            tot_cycles += now_cycles;
        } else {
            tot_cycles += now_cycles - last_cycles;
        }
        last_cycles = now_cycles;
    }

    env::commit(&cycles);
}
