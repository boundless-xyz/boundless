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

use risc0_zkvm::guest::env;

pub fn main() {
    let cycles: u64 = env::read();
    // The nonce is used to ensure that the input is unique
    let nonce: u64 = env::read();
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

    env::commit(&(cycles, nonce));
}
