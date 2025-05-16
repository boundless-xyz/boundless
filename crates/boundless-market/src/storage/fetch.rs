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

//! An implementation of URL fetching that supports the common URL types seen on Boundless.

use anyhow::{bail, ensure};
use url::Url;

/// Fetches the content of a URL.
/// Supported URL schemes are `http`, `https`, and `file`.
pub async fn fetch_url(url_str: impl AsRef<str>) -> anyhow::Result<Vec<u8>> {
    tracing::debug!("Fetching URL: {}", url_str.as_ref());
    let url = Url::parse(url_str.as_ref())?;

    match url.scheme() {
        "http" | "https" => fetch_http(&url).await,
        "file" => {
            ensure!(risc0_zkvm::is_dev_mode(), "file fetch is on enabled when RISC0_DEV_MODE is enabled");
            fetch_file(&url).await
        },
        _ => bail!("unsupported URL scheme: {}", url.scheme()),
    }
}

async fn fetch_http(url: &Url) -> anyhow::Result<Vec<u8>> {
    let response = reqwest::get(url.as_str()).await?;
    let status = response.status();
    if !status.is_success() {
        bail!("HTTP request failed with status: {}", status);
    }

    Ok(response.bytes().await?.to_vec())
}

async fn fetch_file(url: &Url) -> anyhow::Result<Vec<u8>> {
    let path = std::path::Path::new(url.path());
    let data = tokio::fs::read(path).await?;
    Ok(data)
}
