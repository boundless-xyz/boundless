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
use std::collections::HashMap;

#[derive(Serialize, Deserialize)]
pub(crate) struct InputEnvV1 {
    version: u8,
    env_vars: HashMap<String, String>,
    input: Vec<u8>,
}

/// Input builder.
#[derive(Clone, Default, Debug)]
pub struct InputEnv {
    env_vars: HashMap<String, String>,
    input: Vec<u8>,
}

impl InputEnv {
    /// Create a new input builder.
    pub fn new() -> Self {
        Self { env_vars: HashMap::new(), input: Vec::new() }
    }

    /// Access the raw input bytes
    pub fn input(&self) -> Vec<u8> {
        self.input.clone()
    }

    /// Access the environment variables
    pub fn env_vars(&self) -> HashMap<String, String> {
        self.env_vars.clone()
    }

    /// Return the input data packed in MessagePack format
    pub fn build(self) -> Result<Vec<u8>> {
        let v1 = InputEnvV1 { version: 1, env_vars: self.env_vars, input: self.input };

        Ok(rmp_serde::to_vec(&v1)?)
    }

    /// Parse a MessagePack formatted input with version support
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        // Uncomment this block when we have a new version
        // if let Ok(v2) = rmp_serde::from_slice::<InputEnvV2>(bytes) {
        //     match v2.version {
        //         2 => return Ok(Self {
        //             env_vars: v2.env_vars,
        //             input: v2.input,
        //             // extra fields if needed in the future
        //         }),
        //         // fall through
        //         _ => {}
        //     }
        // }
        let v1: InputEnvV1 = rmp_serde::from_slice(bytes)?;
        match v1.version {
            1 => Ok(Self { env_vars: v1.env_vars, input: v1.input }),
            v => bail!("Unsupported version: {}", v),
        }
    }

    /// Add environment variables to the guest environment.
    ///
    /// # Example
    ///
    /// ```
    /// use boundless_market::input::InputEnv;
    /// use std::collections::HashMap;
    ///
    /// let mut vars = HashMap::new();
    /// vars.insert("VAR1".to_string(), "SOME_VALUE".to_string());
    /// vars.insert("VAR2".to_string(), "SOME_VALUE".to_string());
    ///
    /// let input = InputEnv::new()
    ///     .with_env_vars(vars)
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn with_env_vars(self, vars: HashMap<String, String>) -> Self {
        Self { env_vars: vars, ..self }
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
    ///     .build()
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
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn write_slice<T: Pod>(self, slice: &[T]) -> Self {
        let mut input = self.input;
        input.extend_from_slice(bytemuck::cast_slice(slice));
        Self { input, ..self }
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
        Self { input, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_parsing() -> Result<()> {
        // Test V1
        let mut env_vars = HashMap::new();
        env_vars.insert("KEY".to_string(), "VALUE".to_string());
        let v1 = InputEnvV1 { version: 1, env_vars, input: vec![1, 2, 3] };
        let bytes = rmp_serde::to_vec(&v1)?;
        let parsed = InputEnv::from_bytes(&bytes)?;
        assert_eq!(parsed.input(), vec![1, 2, 3]);

        // Test unsupported version
        let v2 = InputEnvV1 { version: 2, env_vars: HashMap::new(), input: Vec::new() };
        let bytes = rmp_serde::to_vec(&v2)?;
        let parsed = InputEnv::from_bytes(&bytes);
        assert!(parsed.is_err());

        Ok(())
    }
}
