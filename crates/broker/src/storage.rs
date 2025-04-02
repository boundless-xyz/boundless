// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

use std::{
    fmt::{Display, Formatter},
    path::PathBuf,
    str::FromStr,
};

use alloy::primitives::bytes::Buf;
use futures::StreamExt;
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use thiserror::Error;

#[derive(Error, Debug)]
#[non_exhaustive]
pub enum StorageErr {
    #[error("Failed to parse URL")]
    UriParseErr(#[from] url::ParseError),

    #[error("Uri unsupported scheme: {0}")]
    UnsupportedScheme(String),

    #[error("Uri contents large than size limit: {0}")]
    TooLarge(usize),

    #[error("Bonsai does not support fetch, use exist() and assume it is present")]
    BonsaiFetch,

    #[error("Http reqwest error")]
    HttpErr(#[from] reqwest::Error),

    #[error("Http reqwest_middleware error")]
    HttpMiddlewareErr(#[from] reqwest_middleware::Error),

    #[error("HTTP status error {0}")]
    HttpStatusErr(String),

    #[error("HTTP fetch failed after {0} retries")]
    FetchRetryMax(u8),

    #[error("Authority missing")]
    AuthorityMissing,

    #[error("Bonsai authority invalid: {0}")]
    InvalidBonsaiHost(String),

    #[error("Uri missing a path component")]
    NullBonsaiPath,
}

pub struct UriHandler {
    uri: url::Url,
    uri_scheme: String,
    max_size: Option<usize>,
    retries: u8,
    client: ClientWithMiddleware,
}

const DEFAULT_RETRY_NUMB: u8 = 1;

impl UriHandler {
    fn supported_scheme(scheme: &str) -> bool {
        if risc0_zkvm::is_dev_mode() {
            return matches!(scheme, "bonsai" | "http" | "https" | "file");
        }
        matches!(scheme, "bonsai" | "http" | "https")
    }

    fn supported_bonsai_host(authority: &str) -> bool {
        matches!(authority, "image" | "input")
    }

    pub fn new(
        uri_str: &str,
        max_size: Option<usize>,
        retries: Option<u8>,
        cache_dir: Option<PathBuf>,
    ) -> Result<Self, StorageErr> {
        let uri = url::Url::parse(uri_str)?;

        let scheme = uri.scheme().to_string();

        if !Self::supported_scheme(&scheme) {
            return Err(StorageErr::UnsupportedScheme(scheme));
        }

        // file scheme is only supported in dev mode
        if scheme == "file" && !risc0_zkvm::is_dev_mode() {
            return Err(StorageErr::UnsupportedScheme(scheme));
        }

        if scheme == "bonsai" {
            let authority = http::uri::Authority::from_str(uri.authority())
                .map_err(|_| StorageErr::AuthorityMissing)?;
            if !Self::supported_bonsai_host(authority.host()) {
                return Err(StorageErr::InvalidBonsaiHost(authority.host().to_string()));
            }

            let path = uri.path();
            if path.is_empty() || path == "/" {
                return Err(StorageErr::NullBonsaiPath);
            }
        }

        let client = if let Some(cache_dir) = cache_dir {
            let manager = CACacheManager { path: cache_dir };
            let cache = Cache(HttpCache {
                mode: CacheMode::Default,
                manager,
                options: HttpCacheOptions::default(),
            });
            ClientBuilder::new(reqwest::Client::new()).with(cache).build()
        } else {
            ClientBuilder::new(reqwest::Client::new()).build()
        };

        Ok(Self {
            uri,
            uri_scheme: scheme,
            max_size,
            retries: retries.unwrap_or(DEFAULT_RETRY_NUMB),
            client,
        })
    }

    pub fn exists(&self) -> bool {
        match self.uri_scheme.as_ref() {
            "file" if !risc0_zkvm::is_dev_mode() => unreachable!(),
            "file" => false,
            "bonsai" => true,
            "http" | "https" => false,
            _ => unreachable!(),
        }
    }

    pub async fn fetch(&self) -> Result<Vec<u8>, StorageErr> {
        match self.uri_scheme.as_ref() {
            "bonsai" => Err(StorageErr::BonsaiFetch),
            "http" | "https" => {
                let mut retry = 0;
                let res = loop {
                    // TODO: move these ?'s to captures + retries
                    // currently only retry on http status code failures
                    let res = self.client.get(self.uri.to_string()).send().await?;
                    let status = res.status();
                    if status.is_success() {
                        break res;
                    } else {
                        let body = res.text().await?;
                        tracing::error!(
                            "HTTP error fetching contents {retry}/{}: {status} - {body}",
                            self.retries
                        );
                        if retry == self.retries {
                            return Err(StorageErr::FetchRetryMax(self.retries));
                        }
                        retry += 1;
                        // TODO configurable...
                        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
                        continue;
                    }
                };

                let mut buffer = vec![];
                if let Some(content_length) = res.content_length() {
                    if let Some(max_size) = self.max_size {
                        if content_length as usize > max_size {
                            return Err(StorageErr::TooLarge(content_length as usize));
                        }
                        buffer.reserve(content_length as usize);
                    }
                }

                let mut resp_stream = res.bytes_stream();
                while let Some(chunk) = resp_stream.next().await {
                    let chunk = chunk?;
                    buffer.extend_from_slice(chunk.chunk());
                    if let Some(max_size) = self.max_size {
                        if buffer.len() > max_size {
                            return Err(StorageErr::TooLarge(buffer.len()));
                        }
                    }
                }

                Ok(buffer)
            }
            // file scheme is only supported in dev mode
            "file" if !risc0_zkvm::is_dev_mode() => {
                Err(StorageErr::UnsupportedScheme(self.uri_scheme.clone()))
            }
            "file" => {
                let path = std::path::Path::new(self.uri.path());
                let data = tokio::fs::read(path)
                    .await
                    .map_err(|_| StorageErr::HttpStatusErr("File not found".to_string()))?;
                Ok(data)
            }
            _ => Err(StorageErr::UnsupportedScheme(self.uri_scheme.clone())),
        }
    }

    pub fn id(&self) -> Result<String, StorageErr> {
        match self.uri_scheme.as_ref() {
            "bonsai" => Ok(self.uri.path()[1..].to_string()),
            _ => Err(StorageErr::UnsupportedScheme(self.uri_scheme.clone())),
        }
    }
}

#[derive(Default)]
pub struct UriHandlerBuilder {
    uri_str: String,
    max_size: Option<usize>,
    retries: Option<u8>,
    cache_dir: Option<PathBuf>,
}

impl UriHandlerBuilder {
    pub fn new(uri_str: &str) -> Self {
        Self { uri_str: uri_str.into(), max_size: None, retries: None, cache_dir: None }
    }

    pub fn set_max_size(mut self, max_size: usize) -> Self {
        self.max_size = Some(max_size);
        self
    }

    pub fn set_retries(mut self, retries: u8) -> Self {
        self.retries = Some(retries);
        self
    }

    pub fn set_cache_dir(mut self, cache_dir: &Option<PathBuf>) -> Self {
        self.cache_dir = cache_dir.clone();
        self
    }

    pub fn build(self) -> Result<UriHandler, StorageErr> {
        UriHandler::new(&self.uri_str, self.max_size, self.retries, self.cache_dir)
    }
}

impl Display for UriHandler {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.uri)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;
    use tracing_test::traced_test;

    #[test]
    fn bonsai_uri_parser() {
        let handler = UriHandlerBuilder::new("bonsai://image/02edb913-c1f5-4ca9-89c0-8ee308b21aef")
            .build()
            .unwrap();
        assert_eq!(handler.id().unwrap(), "02edb913-c1f5-4ca9-89c0-8ee308b21aef");
    }

    #[test]
    #[should_panic(expected = "InvalidBonsaiHost(\"test")]
    fn bonsai_bad_host() {
        UriHandlerBuilder::new("bonsai://test/blah").build().unwrap();
    }

    #[test]
    #[should_panic(expected = "NullBonsaiPath")]
    fn bonsai_missing_path() {
        UriHandlerBuilder::new("bonsai://image").build().unwrap();
    }

    #[test]
    fn bonsai_exists() {
        let uri = UriHandlerBuilder::new("bonsai://image/test").build().unwrap();
        assert!(uri.exists());
    }

    #[test]
    fn http_parse() {
        UriHandlerBuilder::new("http://risczero.com/images/02edb913-c1f5-4ca9-89c0-8ee308b21aef")
            .build()
            .unwrap();
    }

    #[test]
    fn http_exists() {
        assert!(!UriHandlerBuilder::new("https://risczero.com/").build().unwrap().exists());
    }

    #[tokio::test]
    async fn http_fetch() {
        let server = MockServer::start();
        let resp_data = vec![0x41, 0x41, 0x41, 0x41];
        let get_mock = server.mock(|when, then| {
            when.method(GET).path("/image");
            then.status(200).body(&resp_data);
        });

        let url = format!("http://{}/image", server.address());
        let handler = UriHandlerBuilder::new(&url).set_max_size(1_000_000).build().unwrap();
        assert!(!handler.exists());

        let data = handler.fetch().await.unwrap();
        assert_eq!(data, resp_data);
        get_mock.assert();
    }

    #[traced_test]
    #[tokio::test]
    async fn http_fetch_retry() {
        static mut REQ_COUNT: u32 = 0;

        let server = MockServer::start();
        let get_mock = server.mock(|when, then| {
            when.method(GET).path("/image").matches(|_req: &HttpMockRequest| {
                let req = unsafe {
                    let req = REQ_COUNT;
                    REQ_COUNT += 1;
                    req
                };
                req >= 1
            });
            then.status(200).body("TEST");
        });

        let url = format!("http://{}/image", server.address());
        let handler =
            UriHandlerBuilder::new(&url).set_max_size(1_000_000).set_retries(1).build().unwrap();
        assert!(!handler.exists());

        let _data = handler.fetch().await.unwrap();
        get_mock.assert();
        assert!(logs_contain("HTTP error fetching contents 0/1"));
    }

    #[tokio::test]
    #[should_panic(expected = "TooLarge")]
    async fn max_size_limit() {
        let server = MockServer::start();
        let resp_data = vec![0x41, 0x41, 0x41, 0x41];
        let get_mock = server.mock(|when, then| {
            when.method(GET).path("/image");
            then.status(200).body(&resp_data);
        });

        let url = format!("http://{}/image", server.address());
        let handler = UriHandlerBuilder::new(&url).set_max_size(1).build().unwrap();
        assert!(!handler.exists());

        let _data = handler.fetch().await.unwrap();
        get_mock.assert();
    }
}
