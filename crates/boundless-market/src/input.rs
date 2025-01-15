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

use anyhow::Result;
use bytemuck::Pod;
use risc0_zkvm::serde::to_vec;
use serde::Serialize;

/// Input builder.
#[derive(Clone, Default, Debug)]
pub struct InputBuilder {
    input: Vec<u8>,
}

impl InputBuilder {
    /// Create a new input builder.
    pub fn new() -> Self {
        Self { input: Vec::new() }
    }

    /// Return the input data.
    pub fn build(self) -> Vec<u8> {
        self.input
    }

    /// Write input data.
    ///
    /// This function will serialize `data` using a zkVM-optimized codec that
    /// can be deserialized in the guest with a corresponding `risc0_zkvm::env::read` with
    /// the same data type.
    ///
    /// # Example
    ///
    /// ```
    /// use boundless_market::input::InputBuilder;
    /// use serde::Serialize;
    ///
    /// #[derive(Serialize)]
    /// struct Input {
    ///     a: u32,
    ///     b: u32,
    /// }
    ///
    /// let input1 = Input{ a: 1, b: 2 };
    /// let input2 = Input{ a: 3, b: 4 };
    /// let input = InputBuilder::new()
    ///     .write(&input1).unwrap()
    ///     .write(&input2).unwrap()
    ///     .build();
    /// ```
    pub fn write<T: Serialize>(self, data: &T) -> Result<Self> {
        Ok(self.write_slice(&to_vec(data)?))
    }

    /// Write input data.
    ///
    /// This function writes a slice directly to the underlying buffer. A
    /// corresponding `risc0_zkvm::env::read_slice` can be used within
    /// the guest to read the data.
    ///
    /// # Example
    ///
    /// ```
    /// use boundless_market::input::InputBuilder;
    ///
    /// let slice1 = [0, 1, 2, 3];
    /// let slice2 = [3, 2, 1, 0];
    /// let input = InputBuilder::new()
    ///     .write_slice(&slice1)
    ///     .write_slice(&slice2)
    ///     .build();
    /// ```
    pub fn write_slice<T: Pod>(self, slice: &[T]) -> Self {
        let mut input = self.input;
        input.extend_from_slice(bytemuck::cast_slice(slice));
        Self { input }
    }

    /// Write a frame.
    ///
    /// A frame contains a length header along with the payload. Reading a frame
    /// can be more efficient than deserializing a message on-demand. On-demand
    /// deserialization can cause many syscalls, whereas a frame will only have
    /// two.
    pub fn write_frame(self, payload: &[u8]) -> Self {
        let len = payload.len() as u32;
        let mut input = self.input;
        input.extend_from_slice(&len.to_le_bytes());
        input.extend_from_slice(payload);
        Self { input }
    }
}
