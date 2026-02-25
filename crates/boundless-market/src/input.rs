// Copyright 2026 Boundless Foundation, Inc.
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

use std::io::Read as _;

use bytemuck::Pod;
use risc0_zkvm::serde::to_vec;
use risc0_zkvm::ExecutorEnv;
use rmp_serde;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::contracts::RequestInput;

/// Default zstd compression level for V2 encoding.
const ZSTD_COMPRESSION_LEVEL: i32 = 10;

/// Maximum decompressed size for V2 inputs (256 MiB).
const MAX_DECOMPRESSED_SIZE: usize = 256 * 1024 * 1024;

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
    // MessagePack encoded + zstd compressed.
    V2 = 2,
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
            v if v == Version::V2 as u8 => Ok(Version::V2),
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
    /// zstd compression error
    #[error("zstd compression error: {0}")]
    ZstdCompress(std::io::Error),
    /// zstd decompression error
    #[error("zstd decompression error: {0}")]
    ZstdDecompress(std::io::Error),
    /// Decompressed input exceeds maximum size limit.
    #[error("Decompressed input exceeds maximum size limit of {limit} bytes")]
    DecompressionSizeExceeded {
        /// The limit that was exceeded.
        limit: usize,
    },
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
    /// Create a new [GuestEnvBuilder]
    pub fn builder() -> GuestEnvBuilder {
        Default::default()
    }

    /// Parse an encoded [GuestEnv] with version support.
    ///
    /// For V2 (zstd-compressed) inputs, decompression is capped at `MAX_DECOMPRESSED_SIZE`
    /// to protect against decompression bombs. Use [`Self::decode_with_limit`] to override the limit.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        Self::decode_with_limit(bytes, MAX_DECOMPRESSED_SIZE)
    }

    /// Parse an encoded [GuestEnv] with version support and a custom decompression size limit.
    ///
    /// The `max_decompressed_bytes` parameter caps the decompressed output for V2 inputs.
    /// If the decompressed data exceeds this limit, [Error::DecompressionSizeExceeded] is returned.
    pub fn decode_with_limit(bytes: &[u8], max_decompressed_bytes: usize) -> Result<Self, Error> {
        if bytes.is_empty() {
            return Err(Error::EmptyEncodedInput);
        }
        match Version::try_from(bytes[0])? {
            Version::V0 => Ok(Self { stdin: bytes[1..].to_vec() }),
            Version::V1 => Ok(rmp_serde::from_read(&bytes[1..])?),
            Version::V2 => {
                let decoder = zstd::Decoder::new(&bytes[1..]).map_err(Error::ZstdDecompress)?;
                let mut decompressed = Vec::new();
                let limit = max_decompressed_bytes as u64;
                decoder
                    .take(limit + 1)
                    .read_to_end(&mut decompressed)
                    .map_err(Error::ZstdDecompress)?;
                if decompressed.len() as u64 > limit {
                    return Err(Error::DecompressionSizeExceeded { limit: max_decompressed_bytes });
                }
                Ok(rmp_serde::from_read(decompressed.as_slice())?)
            }
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

    /// Encode the [GuestEnv] with zstd compression (V2) for inclusion in a proof request.
    ///
    /// Produces a V2-encoded payload: `[0x02][zstd(msgpack(self))]`.
    /// If the serialized payload exceeds `MAX_DECOMPRESSED_SIZE` (256 MiB), a warning is logged and
    /// the input is returned as uncompressed V1 to avoid producing a decompression bomb.
    pub fn encode_compressed(&self) -> Result<Vec<u8>, Error> {
        self.encode_compressed_with_options(ZSTD_COMPRESSION_LEVEL, MAX_DECOMPRESSED_SIZE)
    }

    /// Encode with zstd compression using a custom compression level.
    ///
    /// See [`Self::encode_compressed`] for details. The `level` parameter controls the zstd
    /// compression level (higher = better ratio, slower encode).
    pub fn encode_compressed_with_level(&self, level: i32) -> Result<Vec<u8>, Error> {
        self.encode_compressed_with_options(level, MAX_DECOMPRESSED_SIZE)
    }

    /// Encode with zstd compression using custom compression level and size limit.
    ///
    /// If the serialized msgpack payload exceeds `max_size` bytes, a warning is logged and
    /// the input falls back to uncompressed V1 encoding to avoid producing a decompression bomb.
    pub fn encode_compressed_with_options(
        &self,
        level: i32,
        max_size: usize,
    ) -> Result<Vec<u8>, Error> {
        let msgpack = rmp_serde::to_vec_named(self)?;
        if msgpack.len() > max_size {
            warn!(
                msgpack_len = msgpack.len(),
                max_size,
                "serialized input exceeds max decompressed size, falling back to uncompressed V1"
            );
            return self.encode();
        }
        let compressed =
            zstd::encode_all(msgpack.as_slice(), level).map_err(Error::ZstdCompress)?;
        let mut encoded = Vec::with_capacity(1 + compressed.len());
        encoded.push(Version::V2.into());
        encoded.extend_from_slice(&compressed);
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

/// Options for zstd compression (V2 encoding).
///
/// When provided to [GuestEnvBuilder], enables V2 (MessagePack + zstd) encoding.
#[derive(Clone, Debug)]
pub struct CompressionOptions {
    /// Zstd compression level. Higher values produce better compression at the cost of slower
    /// encoding. Defaults to [`ZSTD_COMPRESSION_LEVEL`] (10).
    pub level: i32,

    /// Maximum serialized input size before compression falls back to uncompressed V1.
    /// Defaults to [`MAX_DECOMPRESSED_SIZE`] (256 MiB).
    pub max_input_size: usize,
}

impl Default for CompressionOptions {
    fn default() -> Self {
        Self { level: ZSTD_COMPRESSION_LEVEL, max_input_size: MAX_DECOMPRESSED_SIZE }
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

    /// Compression options. When `Some`, `build_vec` and `build_inline` produce V2
    /// (zstd-compressed) encoding. When `None`, uncompressed V1 encoding is used.
    pub compression: Option<CompressionOptions>,
}

impl GuestEnvBuilder {
    /// Create a new input builder.
    pub fn new() -> Self {
        Self { stdin: Vec::new(), compression: None }
    }

    /// Enable zstd compression with default options for the encoded output.
    ///
    /// When set, [`Self::build_vec`] and [`Self::build_inline`] will produce V2 (MessagePack + zstd) encoding.
    pub fn with_compression(self) -> Self {
        let opts = self.compression.unwrap_or_default();
        Self { compression: Some(opts), ..self }
    }

    /// Set a custom zstd compression level (default: 10).
    ///
    /// Higher values produce better compression at the cost of slower encoding.
    /// Enables compression if not already enabled.
    pub fn with_compression_level(self, level: i32) -> Self {
        let opts = self.compression.unwrap_or_default();
        Self { compression: Some(CompressionOptions { level, ..opts }), ..self }
    }

    /// Set a custom maximum input size for compressed encoding (default: 256 MiB).
    ///
    /// If the serialized payload exceeds this limit, encoding falls back to uncompressed V1.
    /// Enables compression if not already enabled.
    pub fn with_max_compressed_input_size(self, max: usize) -> Self {
        let opts = self.compression.unwrap_or_default();
        Self { compression: Some(CompressionOptions { max_input_size: max, ..opts }), ..self }
    }

    /// Build the [GuestEnv] for inclusion in a proof request.
    pub fn build_env(self) -> GuestEnv {
        GuestEnv { stdin: self.stdin }
    }

    /// Build and encode [GuestEnv] for inclusion in a proof request.
    pub fn build_vec(self) -> Result<Vec<u8>, Error> {
        let Self { stdin, compression } = self;
        let env = GuestEnv { stdin };
        match compression {
            Some(opts) => env.encode_compressed_with_options(opts.level, opts.max_input_size),
            None => env.encode(),
        }
    }

    /// Build and encode the [GuestEnv] into an inline [RequestInput] for inclusion in a proof request.
    pub fn build_inline(self) -> Result<RequestInput, Error> {
        let Self { stdin, compression } = self;
        let env = GuestEnv { stdin };
        let encoded = match compression {
            Some(opts) => env.encode_compressed_with_options(opts.level, opts.max_input_size)?,
            None => env.encode()?,
        };
        Ok(RequestInput::inline(encoded))
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
    /// use boundless_market::GuestEnv;
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
    /// let input = GuestEnv::builder()
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
    /// use boundless_market::GuestEnv;
    ///
    /// let slice1 = [0, 1, 2, 3];
    /// let slice2 = [3, 2, 1, 0];
    /// let input = GuestEnv::builder()
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
        let v1 = GuestEnv::builder().write_slice(&[1u8, 2, 3]);
        let bytes = v1.build_vec()?;
        let parsed = GuestEnv::decode(&bytes)?;
        assert_eq!(parsed.stdin, vec![1, 2, 3]);

        // Test V0
        let bytes = vec![0u8, 1, 2, 3];
        let parsed = GuestEnv::decode(&bytes)?;
        assert_eq!(parsed.stdin, vec![1, 2, 3]);

        // Test unsupported version
        let bytes = vec![255u8, 1, 2, 3];
        let parsed = GuestEnv::decode(&bytes);
        assert!(parsed.is_err());

        Ok(())
    }

    #[test]
    fn test_encode_decode_env() -> Result<(), Error> {
        let timestamp = format! {"{:?}", std::time::SystemTime::now()};
        let env = GuestEnv::builder().write_slice(timestamp.as_bytes()).build_env();

        let decoded_env = GuestEnv::decode(&env.encode()?)?;
        assert_eq!(env, decoded_env);
        Ok(())
    }

    #[test]
    fn test_v2_round_trip() -> Result<(), Error> {
        let env = GuestEnv::from_stdin(b"hello, compressed world!".to_vec());
        let compressed = env.encode_compressed()?;
        assert_eq!(compressed[0], 2u8); // Version::V2
        let decoded = GuestEnv::decode(&compressed)?;
        assert_eq!(decoded, env);
        Ok(())
    }

    #[test]
    fn test_v2_compresses_data() -> Result<(), Error> {
        // Highly compressible: repeated bytes
        let data = vec![0x42u8; 10_000];
        let env = GuestEnv::from_stdin(data);
        let v1 = env.encode()?;
        let v2 = env.encode_compressed()?;
        assert!(
            v2.len() < v1.len(),
            "V2 should be smaller for compressible data: v1={}, v2={}",
            v1.len(),
            v2.len()
        );
        // Both decode to the same thing
        assert_eq!(GuestEnv::decode(&v1)?, GuestEnv::decode(&v2)?);
        Ok(())
    }

    #[test]
    fn test_v2_empty_stdin() -> Result<(), Error> {
        let env = GuestEnv::from_stdin(vec![]);
        let compressed = env.encode_compressed()?;
        let decoded = GuestEnv::decode(&compressed)?;
        assert_eq!(decoded, env);
        Ok(())
    }

    #[test]
    fn test_v2_small_data() -> Result<(), Error> {
        let env = GuestEnv::from_stdin(vec![1, 2, 3]);
        let compressed = env.encode_compressed()?;
        let decoded = GuestEnv::decode(&compressed)?;
        assert_eq!(decoded, env);
        Ok(())
    }

    #[test]
    fn test_v2_corrupt_data() {
        let bytes = vec![2u8, 0xFF, 0xFF, 0xFF]; // Version V2 + garbage
        let result = GuestEnv::decode(&bytes);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::ZstdDecompress(_)));
    }

    #[test]
    fn test_v2_decompression_bomb_protection() -> Result<(), Error> {
        // Compress a payload that decompresses to more than our test limit
        let large_data = vec![0u8; 1024];
        let env = GuestEnv::from_stdin(large_data);
        let compressed = env.encode_compressed()?;

        // Decode with a limit smaller than the decompressed size
        let result = GuestEnv::decode_with_limit(&compressed, 16);
        assert!(
            matches!(result, Err(Error::DecompressionSizeExceeded { limit: 16 })),
            "Expected DecompressionSizeExceeded, got: {result:?}"
        );

        // Decode with a sufficient limit should succeed
        let decoded = GuestEnv::decode_with_limit(&compressed, 64 * 1024)?;
        assert_eq!(decoded, env);

        Ok(())
    }

    #[test]
    fn test_builder_with_compression() -> Result<(), Error> {
        let bytes = GuestEnv::builder().write_slice(&[1u8, 2, 3]).with_compression().build_vec()?;
        assert_eq!(bytes[0], 2u8); // V2
        let decoded = GuestEnv::decode(&bytes)?;
        assert_eq!(decoded.stdin, vec![1, 2, 3]);
        Ok(())
    }

    #[test]
    fn test_builder_default_no_compression() -> Result<(), Error> {
        let bytes = GuestEnv::builder().write_slice(&[1u8, 2, 3]).build_vec()?;
        assert_eq!(bytes[0], 1u8); // V1
        Ok(())
    }

    #[test]
    fn test_builder_custom_compression_level() -> Result<(), Error> {
        let data = vec![0x42u8; 1_000];
        let bytes =
            GuestEnv::builder().write_slice(&data).with_compression_level(19).build_vec()?;
        assert_eq!(bytes[0], 2u8); // V2
        let decoded = GuestEnv::decode(&bytes)?;
        assert_eq!(decoded.stdin, data);
        Ok(())
    }

    #[test]
    fn test_encode_compressed_fallback_on_oversize() -> Result<(), Error> {
        // Use a tiny max_size so the fallback triggers
        let env = GuestEnv::from_stdin(vec![0u8; 128]);
        let encoded = env.encode_compressed_with_options(ZSTD_COMPRESSION_LEVEL, 16)?;
        assert_eq!(encoded[0], 1u8, "should fall back to V1 when payload exceeds max_size");
        let decoded = GuestEnv::decode(&encoded)?;
        assert_eq!(decoded, env);
        Ok(())
    }

    #[test]
    fn test_builder_custom_max_compressed_input_size() -> Result<(), Error> {
        let data = vec![0u8; 128];
        let bytes = GuestEnv::builder()
            .write_slice(&data)
            .with_compression()
            .with_max_compressed_input_size(16)
            .build_vec()?;
        assert_eq!(bytes[0], 1u8, "should fall back to V1 when exceeding custom limit");
        let decoded = GuestEnv::decode(&bytes)?;
        assert_eq!(decoded.stdin, data);
        Ok(())
    }
}
