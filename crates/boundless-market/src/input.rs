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

use bytemuck::Pod;
use risc0_zkvm::serde::to_vec;
use rmp_serde;
use serde::{Deserialize, Serialize};

// Input version.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
enum Version {
    // Raw version with no encoding.
    V0 = 0,
    // MessagePack encoded version based on [InputEnvV1].
    #[default]
    V1 = 1,
}

impl From<Version> for u8 {
    fn from(v: Version) -> Self {
        v as u8
    }
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
/// Input error.
pub enum Error {
    /// MessagePack serde encoding error
    #[error("MessagePack serde encoding error: {0}")]
    MessagePackSerdeEncode(#[from] rmp_serde::encode::Error),
    /// MessagePack serde decoding error
    #[error("MessagePack serde decoding error: {0}")]
    MessagePackSerdeDecode(#[from] rmp_serde::decode::Error),
    /// risc0-zkvm Serde error
    #[error("risc0-zkvm Serde error: {0}")]
    ZkvmSerde(#[from] risc0_zkvm::serde::Error),
    /// Serde json error
    #[error("serde_json error: {0}")]
    SerdeJson(#[from] serde_json::Error),
    /// Invalid version
    #[error("Missing or invalid version")]
    InvalidVersion,
    /// Unsupported version
    #[error("Unsupported version: {0}")]
    UnsupportedVersion(u64),
    /// Invalid input
    #[error("Invalid input")]
    InvalidInput,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct InputEnvV1 {
    stdin: Vec<u8>,
}

/// Input builder.
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct InputEnv {
    version: Version,
    /// Input data.
    pub stdin: Vec<u8>,
}

impl InputEnv {
    /// Create a new input builder.
    pub fn new() -> Self {
        Self { version: Version::V1, stdin: Vec::new() }
    }

    /// Return the input data packed in MessagePack format
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let v1 = InputEnvV1 { stdin: self.stdin.clone() };
        let mut encoded = Vec::new();
        encoded.push(self.version.into());
        encoded.extend_from_slice(&rmp_serde::to_vec_named(&v1)?);
        Ok(encoded)
    }

    /// Parse a MessagePack formatted input with version support
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.is_empty() {
            return Err(Error::InvalidInput);
        }
        match bytes[0] {
            0 => Ok(Self { version: Version::V0, stdin: bytes[1..].to_vec() }),
            1 => {
                let v1: InputEnvV1 = rmp_serde::from_read(&bytes[1..])?;
                Ok(Self { version: Version::V1, stdin: v1.stdin })
            }
            _ => Err(Error::UnsupportedVersion(bytes[0] as u64)),
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
    ///     .write(&input2).unwrap();
    /// ```
    pub fn write<T: Serialize>(self, data: &T) -> Result<Self, Error> {
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
    ///     .write_slice(&slice2);
    /// ```
    pub fn write_slice<T: Pod>(self, slice: &[T]) -> Self {
        let mut input = self.stdin;
        input.extend_from_slice(bytemuck::cast_slice(slice));
        Self { stdin: input, ..self }
    }

    /// Write a frame.
    ///
    /// A frame contains a length header along with the payload. Reading a frame
    /// can be more efficient than deserializing a message on-demand. On-demand
    /// deserialization can cause many syscalls, whereas a frame will only have
    /// two.
    pub fn write_frame(self, payload: &[u8]) -> Self {
        let len = payload.len() as u32;
        let mut input = self.stdin;
        input.extend_from_slice(&len.to_le_bytes());
        input.extend_from_slice(payload);
        Self { stdin: input, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parsing() -> Result<(), Error> {
        // Test V1
        let v1 = InputEnv::new().write_slice(&[1u8, 2, 3]);
        let bytes = v1.encode()?;
        let parsed = InputEnv::decode(&bytes)?;
        assert_eq!(parsed.stdin, vec![1, 2, 3]);

        // Test unsupported version
        let bytes = vec![2u8, 1, 2, 3];
        let parsed = InputEnv::decode(&bytes);
        assert!(parsed.is_err());

        Ok(())
    }

    #[test]
    fn test_encode_decode_input() -> Result<(), Error> {
        let timestamp = format! {"{:?}", std::time::SystemTime::now()};
        let encoded_input = InputEnv::new().write_slice(timestamp.as_bytes()).stdin;
        println!("encoded_input: {:?}", hex::encode(&encoded_input));

        let packed_input = InputEnv::new().write_slice(&encoded_input).encode()?;
        println!("packed_input: {:?}", hex::encode(&packed_input));

        let decoded_input = InputEnv::decode(&packed_input)?.stdin;
        assert_eq!(encoded_input, decoded_input);
        Ok(())
    }
}
