// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

use alloy::primitives::bytes::Buf;
use futures::StreamExt;
use thiserror::Error;

#[derive(Error, Debug)]
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

    #[error("HTTP status error {0}")]
    HttpStatusErr(String),

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
}

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

    pub fn new(uri_str: &str, max_size: Option<usize>) -> Result<Self, StorageErr> {
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

        Ok(Self { uri, uri_scheme: scheme, max_size })
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
                let res = reqwest::get(self.uri.to_string()).await?;
                let status = res.status();
                if !status.is_success() {
                    let body = res.text().await?;
                    return Err(StorageErr::HttpStatusErr(format!(
                        "HTTP fetch err: {} - {}",
                        status, body
                    )));
                }

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
}

impl UriHandlerBuilder {
    pub fn new(uri_str: &str) -> Self {
        Self { uri_str: uri_str.into(), max_size: None }
    }

    pub fn set_max_size(mut self, max_size: usize) -> Self {
        self.max_size = Some(max_size);
        self
    }

    pub fn build(self) -> Result<UriHandler, StorageErr> {
        UriHandler::new(&self.uri_str, self.max_size)
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
        let handler = UriHandlerBuilder::new("https://risczero.com/")
            .set_max_size(1_000_000)
            .build()
            .unwrap();
        assert!(!handler.exists());

        let _data = handler.fetch().await.unwrap();
    }

    #[tokio::test]
    #[should_panic(expected = "TooLarge")]
    async fn max_size_limit() {
        let handler =
            UriHandlerBuilder::new("https://risczero.com/").set_max_size(1).build().unwrap();
        assert!(!handler.exists());

        let _data = handler.fetch().await.unwrap();
    }
}
