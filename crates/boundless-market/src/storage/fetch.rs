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

//! An implementation of URL fetching that supports the common URL types seen on Boundless.

use crate::util::is_dev_mode;
use anyhow::{bail, ensure};
use url::Url;

/// The default IPFS gateway URL used by Boundless if no override is provided via
/// the `IPFS_GATEWAY_URL` environment variable.
pub const BOUNDLESS_IPFS_GATEWAY_URL: &str = "https://gateway.beboundless.cloud";

/// Fetches the content of a URL.
/// Supported URL schemes are `http`, `https`, and `file`.
pub async fn fetch_url(url_str: impl AsRef<str>) -> anyhow::Result<Vec<u8>> {
    tracing::debug!("Fetching URL: {}", url_str.as_ref());
    let url = Url::parse(url_str.as_ref())?;

    match url.scheme() {
        "http" | "https" => fetch_http(&url).await,
        "file" => {
            ensure!(
                is_dev_mode() || allow_local_file_storage(),
                "file fetch is only enabled when RISC0_DEV_MODE is enabled"
            );
            fetch_file(&url).await
        }
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

/// Overrides the IPFS gateway in a given URL if it contains `/ipfs/` and does not start with `file://`.
/// The new gateway URL is taken from the `IPFS_GATEWAY_URL` environment variable,
/// or defaults to `BOUNDLESS_IPFS_GATEWAY_URL` if the variable is not set
pub fn override_gateway(url: &str) -> String {
    if !url.contains("/ipfs/")
        || url.starts_with("file://")
        || url.starts_with(BOUNDLESS_IPFS_GATEWAY_URL)
    {
        return url.to_string();
    }

    let parts: Vec<&str> = url.splitn(2, "/ipfs/").collect();
    if parts.len() != 2 {
        return url.to_string();
    }

    let gateway_url =
        std::env::var("IPFS_GATEWAY_URL").unwrap_or(BOUNDLESS_IPFS_GATEWAY_URL.to_string());
    let new_url = format!("{gateway_url}/ipfs/{}", parts[1]);
    new_url
}

/// Returns `true` if the `ALLOW_LOCAL_FILE_STORAGE` environment variable is enabled.
pub(crate) fn allow_local_file_storage() -> bool {
    std::env::var("ALLOW_LOCAL_FILE_STORAGE")
        .ok()
        .map(|x| x.to_lowercase())
        .filter(|x| x == "1" || x == "true" || x == "yes")
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_override_gateway() {
        let original_url = "https://example.com/ipfs/QmHash";
        let overridden_url = override_gateway(original_url);
        assert_eq!(overridden_url, format!("{BOUNDLESS_IPFS_GATEWAY_URL}/ipfs/QmHash"));
        let file_url = "file:///path/to/ipfs/QmHash";
        let overridden_file_url = override_gateway(file_url);
        assert_eq!(overridden_file_url, file_url);
        let non_ipfs_url = "https://example.com/some/other/path";
        let overridden_non_ipfs_url = override_gateway(non_ipfs_url);
        assert_eq!(overridden_non_ipfs_url, non_ipfs_url);
    }
}
