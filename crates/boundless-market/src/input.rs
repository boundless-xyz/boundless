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
use risc0_zkvm::ExecutorEnv;
use rmp_serde;
use serde::{Deserialize, Serialize};

use crate::contracts::Input;

// Input version.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
enum Version {
    // Raw version with no encoding.
    V0 = 0,
    // MessagePack encoded version based on [InputV1].
    #[default]
    V1 = 1,
}

impl From<Version> for u8 {
    fn from(v: Version) -> Self {
        v as u8
    }
}

impl TryFrom<u8> for Version {
    type Error = Error;

    fn try_from(v: u8) -> Result<Version, Self::Error> {
        match v {
            v if v == Version::V0 as u8 => Ok(Version::V0),
            v if v == Version::V1 as u8 => Ok(Version::V1),
            _ => Err(Error::UnsupportedVersion(v as u64)),
        }
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
    /// Unsupported version
    #[error("Unsupported version: {0}")]
    UnsupportedVersion(u64),
    /// Encoded input buffer is empty, which is an invalid encoding.
    #[error("Cannot decode empty buffer as input")]
    EmptyEncodedInput,
}

/// Structured input used by the Boundless prover to execute the guest for the proof request.
///
/// This struct is related to the [ExecutorEnv] in that both represent the environments provided to
/// the guest by the host that is executing and proving the execution. In contrast to the
/// [ExecutorEnv] provided by [risc0_zkvm], this struct contains only the options that are
/// supported by Boundless.
#[derive(Clone, Default, Debug, Serialize, Deserialize)]
#[cfg_attr(test, derive(PartialEq, Eq))]
#[non_exhaustive]
pub struct GuestEnv {
    /// Input data to be provided to the guest as stdin.
    ///
    /// The data here will be provided to the guest without further encoding (e.g. the bytes will
    /// be provided directly). When the guest calls `env::read_slice` these are the bytes that will
    /// be read. If the guest uses `env::read`, this should be encoded using the default RISC Zero
    /// codec. [GuestEnvBuilder::write] will encode the data given using the default codec.
    pub stdin: Vec<u8>,
}

impl GuestEnv {
    /// Parse an encoded [GuestEnv] with version support.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.is_empty() {
            return Err(Error::EmptyEncodedInput);
        }
        match Version::try_from(bytes[0])? {
            Version::V0 => Ok(Self { stdin: bytes[1..].to_vec() }),
            Version::V1 => Ok(rmp_serde::from_read(&bytes[1..])?),
        }
    }

    /// Encode the [GuestEnv] for inclusion in a proof request.
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut encoded = Vec::<u8>::new();
        // Push the version as the first byte to indicate the message version.
        encoded.push(Version::V1.into());
        encoded.extend_from_slice(&rmp_serde::to_vec_named(&self)?);
        Ok(encoded)
    }

    /// Create a [GuestEnv] with `stdin` set to the contents of the given `bytes`.
    pub fn from_stdin(bytes: impl Into<Vec<u8>>) -> Self {
        GuestEnv { stdin: bytes.into() }
    }
}

impl TryFrom<GuestEnv> for ExecutorEnv<'_> {
    type Error = anyhow::Error;

    /// Create an [ExecutorEnv], which can be used for execution and proving through the
    /// [risc0_zkvm] [Prover][risc0_zkvm::Prover] and [Executor][risc0_zkvm::Executor] traits, from
    /// the given [GuestEnv].
    fn try_from(env: GuestEnv) -> Result<Self, Self::Error> {
        ExecutorEnv::builder().write_slice(&env.stdin).build()
    }
}

impl From<GuestEnvBuilder> for GuestEnv {
    fn from(builder: GuestEnvBuilder) -> Self {
        builder.build_env()
    }
}

/// Input builder, used to build the structured input (i.e. env) for execution and proving.
///
/// Boundless provers decode the input provided in a proving request as a [GuestEnv]. This
/// [GuestEnvBuilder] provides methods for constructing and encoding the guest environment.
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct GuestEnvBuilder {
    /// Input data to be provided to the guest as stdin.
    ///
    /// See [GuestEnv::stdin]
    pub stdin: Vec<u8>,
}

impl GuestEnvBuilder {
    /// Create a new input builder.
    pub fn new() -> Self {
        Self { stdin: Vec::new() }
    }

    /// Build the [GuestEnv] for inclusion in a proof request.
    pub fn build_env(self) -> GuestEnv {
        GuestEnv { stdin: self.stdin }
    }

    /// Build the and encode [GuestEnv] for inclusion in a proof request.
    pub fn build_vec(self) -> Result<Vec<u8>, Error> {
        self.build_env().encode()
    }

    /// Build and encode the [GuestEnv] into an inline [Input] for inclusion in a proof request.
    pub fn build_inline(self) -> Result<Input, Error> {
        Ok(Input::inline(self.build_env().encode()?))
    }

    /// Write input data.
    ///
    /// This function will serialize `data` using the RISC Zero default codec that
    /// can be deserialized in the guest with a corresponding `risc0_zkvm::env::read` with
    /// the same data type.
    ///
    /// # Example
    ///
    /// ```
    /// use boundless_market::input::GuestEnvBuilder;
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
    /// let input = GuestEnvBuilder::new()
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
    /// use boundless_market::input::GuestEnvBuilder;
    ///
    /// let slice1 = [0, 1, 2, 3];
    /// let slice2 = [3, 2, 1, 0];
    /// let input = GuestEnvBuilder::new()
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
    /// A frame contains a length header along with the payload. Reading a frame can be more
    /// efficient than streaming deserialization of a message. Streaming deserialization
    /// deserialization can cause many syscalls, whereas a frame will only have two.
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
        let v1 = GuestEnvBuilder::new().write_slice(&[1u8, 2, 3]);
        let bytes = v1.build_vec()?;
        let parsed = GuestEnv::decode(&bytes)?;
        assert_eq!(parsed.stdin, vec![1, 2, 3]);

        // Test V0
        let bytes = vec![0u8, 1, 2, 3];
        let parsed = GuestEnv::decode(&bytes)?;
        assert_eq!(parsed.stdin, vec![1, 2, 3]);

        // Test unsupported version
        let bytes = vec![2u8, 1, 2, 3];
        let parsed = GuestEnv::decode(&bytes);
        assert!(parsed.is_err());

        Ok(())
    }

    #[test]
    fn test_encode_decode_env() -> Result<(), Error> {
        let timestamp = format! {"{:?}", std::time::SystemTime::now()};
        let env = GuestEnvBuilder::new().write_slice(timestamp.as_bytes()).build_env();

        let decoded_env = GuestEnv::decode(&env.encode()?)?;
        assert_eq!(env, decoded_env);
        Ok(())
    }
}
