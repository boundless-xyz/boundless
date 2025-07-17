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

//! Provider implementation for storing programs and inputs locally as temporary files.

use std::{fmt::Debug, path::PathBuf, result::Result::Ok, sync::Arc};

use anyhow::anyhow;
use async_trait::async_trait;
use reqwest::Url;
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

use super::{StorageProvider, StorageProviderConfig};

#[derive(Clone, Debug)]
/// Storage provider that uploads ELFs and inputs to a temporary directory.
pub struct TempFileStorageProvider {
    temp_dir: Arc<TempDir>,
}

#[derive(thiserror::Error, Debug)]
/// Error type for the temporary file storage provider.
pub enum TempFileStorageProviderError {
    /// Error type for IO errors.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Error type for URL parsing errors.
    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    /// Error type for other errors.
    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

impl TempFileStorageProvider {
    /// Creates a new temporary file storage provider.
    pub fn new() -> Result<Self, TempFileStorageProviderError> {
        Ok(Self { temp_dir: Arc::new(tempfile::tempdir()?) })
    }

    /// Creates a new temporary file storage provider from the given parts.
    pub fn from_parts(path: &PathBuf) -> Result<Self, TempFileStorageProviderError> {
        Ok(Self { temp_dir: Arc::new(tempfile::tempdir_in(path)?) })
    }

    /// Creates a new temporary file storage provider from the given configuration.
    pub fn from_config(
        config: &StorageProviderConfig,
    ) -> Result<Self, TempFileStorageProviderError> {
        Ok(match &config.file_path {
            Some(path) => Self::from_parts(path)?,
            None => Self::new()?,
        })
    }

    async fn save_file(
        &self,
        data: impl AsRef<[u8]>,
        filename: &str,
    ) -> Result<Url, TempFileStorageProviderError> {
        let file_path = self.temp_dir.path().join(filename);
        tokio::fs::write(&file_path, data.as_ref()).await?;

        let file_url = Url::from_file_path(&file_path)
            .map_err(|()| anyhow!("failed to convert file path to URL: {:?}", file_path))?;
        Ok(file_url)
    }
}

#[async_trait]
impl StorageProvider for TempFileStorageProvider {
    type Error = TempFileStorageProviderError;

    async fn upload_program(&self, program: &[u8]) -> Result<Url, Self::Error> {
        let image_id = risc0_zkvm::compute_image_id(program)?;
        let filename = format!("{image_id}.bin");
        let file_url = self.save_file(program, &filename).await?;
        Ok(file_url)
    }

    async fn upload_input(&self, input: &[u8]) -> Result<Url, Self::Error> {
        let digest = Sha256::digest(input);
        let filename = format!("{}.input", hex::encode(digest.as_slice()));
        let file_url = self.save_file(input, &filename).await?;
        Ok(file_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_temp_file_storage_provider() {
        let provider = TempFileStorageProvider::new().unwrap();

        let program_data = boundless_market_test_utils::ECHO_ELF;
        let input_data = b"test input data";

        let program_url = provider.upload_program(program_data).await.unwrap();
        let input_url = provider.upload_input(input_data).await.unwrap();

        println!("Program URL: {program_url}");
        println!("Input URL: {input_url}");
    }
}
