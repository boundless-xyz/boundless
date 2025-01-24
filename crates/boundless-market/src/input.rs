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

use anyhow::{bail, Result};
use bytemuck::Pod;
use risc0_zkvm::serde::to_vec;
use rmp_serde;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct InputEnvV1 {
    version: u8,
    input: Vec<u8>,
}

/// Input builder.
#[derive(Clone, Default, Debug)]
pub struct InputEnv {
    input: Vec<u8>,
}

impl InputEnv {
    /// Create a new input builder.
    pub fn new() -> Self {
        Self { input: Vec::new() }
    }

    /// Access the raw input bytes
    pub fn input(&self) -> Vec<u8> {
        self.input.clone()
    }

    /// Return the input data packed in MessagePack format
    pub fn pack(self) -> Result<Vec<u8>> {
        let v1 = InputEnvV1 { version: 1, input: self.input };

        Ok(rmp_serde::to_vec(&v1)?)
    }

    /// Parse a MessagePack formatted input with version support
    pub fn unpack(bytes: &[u8]) -> Result<Self> {
        // Uncomment this block when we have a new version
        // if let Ok(v2) = rmp_serde::from_slice::<InputEnvV2>(bytes) {
        //     match v2.version {
        //         2 => return Ok(Self {
        //             input: v2.input,
        //             // extra fields if needed in the future
        //         }),
        //         // fall through
        //         _ => {}
        //     }
        // }
        let v1: InputEnvV1 = rmp_serde::from_slice(bytes)?;
        match v1.version {
            1 => Ok(Self { input: v1.input }),
            v => bail!("Unsupported version: {}", v),
        }
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
    /// use boundless_market::input::InputEnv;
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
    /// let input = InputEnv::new()
    ///     .write(&input1).unwrap()
    ///     .write(&input2).unwrap()
    ///     .unwrap();
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
    /// use boundless_market::input::InputEnv;
    ///
    /// let slice1 = [0, 1, 2, 3];
    /// let slice2 = [3, 2, 1, 0];
    /// let input = InputEnv::new()
    ///     .write_slice(&slice1)
    ///     .write_slice(&slice2)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parsing() -> Result<()> {
        // Test V1
        let v1 = InputEnvV1 { version: 1, input: vec![1, 2, 3] };
        let bytes = rmp_serde::to_vec(&v1)?;
        let parsed = InputEnv::unpack(&bytes)?;
        assert_eq!(parsed.input(), vec![1, 2, 3]);

        // Test unsupported version
        let v2 = InputEnvV1 { version: 2, input: Vec::new() };
        let bytes = rmp_serde::to_vec(&v2)?;
        let parsed = InputEnv::unpack(&bytes);
        assert!(parsed.is_err());

        Ok(())
    }

    #[test]
    fn test_encode_decode_input() -> Result<()> {
        let timestamp = format! {"{:?}", std::time::SystemTime::now()};
        let encoded_input = InputEnv::new().write_slice(timestamp.as_bytes()).input();
        println!("encoded_input: {:?}", hex::encode(&encoded_input));

        let packed_input = InputEnv::new().write_slice(&encoded_input).pack()?;
        println!("packed_input: {:?}", hex::encode(&packed_input));

        let decoded_input = InputEnv::unpack(&packed_input)?.input();
        assert_eq!(encoded_input, decoded_input);
        Ok(())
    }
}
