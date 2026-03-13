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

use alloy::transports::{
    layers::{RateLimitRetryPolicy, RetryPolicy},
    TransportError, TransportErrorKind,
};
use std::time::Duration;

#[derive(Debug, Copy, Clone, Default)]
pub struct CustomRetryPolicy;

/// The retry policy for the RPC provider used throughout
///
/// Extends the default `RateLimitRetryPolicy` with additional retryable cases:
/// - OS error 104 / connection reset by peer
///   https://github.com/boundless-xyz/boundless/issues/240
/// - HTTP 408 (Request Timeout), 500 (Internal Server Error), 502 (Bad Gateway),
///   504 (Gateway Timeout) — standard transient HTTP errors not covered by alloy
/// - JSON-RPC -32601 (Method Not Supported) — transient with aggregating providers
///   like dRPC that fan out to multiple backends; retrying may hit a capable backend
/// - JSON-RPC -32603 (Internal Error) — standard code used for transient server failures
impl RetryPolicy for CustomRetryPolicy {
    fn should_retry(&self, error: &TransportError) -> bool {
        let should_retry = match error {
            TransportError::Transport(TransportErrorKind::Custom(err)) => {
                // easier to match against the debug format string because this is what we see in the logs
                let err_debug_str = format!("{err:?}");
                err_debug_str.contains("os error 104")
                    || err_debug_str.contains("reset by peer")
                    || err_debug_str.contains("operation timed out")
            }
            TransportError::Transport(TransportErrorKind::HttpError(err)) => {
                matches!(err.status, 408 | 500 | 502 | 504)
            }
            TransportError::ErrorResp(err) => {
                matches!(err.code, -32601 | -32603)
            }
            _ => false,
        };
        should_retry || RateLimitRetryPolicy::default().should_retry(error)
    }

    fn backoff_hint(&self, error: &TransportError) -> Option<Duration> {
        RateLimitRetryPolicy::default().backoff_hint(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::transports::{layers::RetryPolicy, HttpError, RpcError, TransportErrorKind};
    use std::fmt;

    struct MockOsError104;

    impl fmt::Debug for MockOsError104 {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("reqwest::Error { kind: Request, source: hyper_util::client::legacy::Error(SendRequest, hyper::Error(Io, Os { code: 104, kind: ConnectionReset, message: \"Connection reset by peer\" })) }")
        }
    }

    struct MockTimeout;

    impl fmt::Debug for MockTimeout {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("reqwest::Error { kind: Request, source: operation timed out }")
        }
    }

    impl fmt::Display for MockTimeout {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Mock Timeout")
        }
    }

    impl std::error::Error for MockTimeout {}

    impl fmt::Display for MockOsError104 {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Mock Error")
        }
    }

    impl std::error::Error for MockOsError104 {}

    fn http_error(status: u16) -> TransportError {
        RpcError::Transport(TransportErrorKind::HttpError(HttpError {
            status,
            body: String::new(),
        }))
    }

    fn jsonrpc_error(code: i64) -> TransportError {
        RpcError::ErrorResp(alloy::rpc::json_rpc::ErrorPayload {
            code,
            message: "error".into(),
            data: None,
        })
    }

    #[test]
    fn retries_on_os_error_104() {
        let policy = CustomRetryPolicy;
        let error = RpcError::Transport(TransportErrorKind::Custom(Box::new(MockOsError104)));
        assert!(policy.should_retry(&error));
    }

    #[test]
    fn retries_on_timeout() {
        let policy = CustomRetryPolicy;
        let error = RpcError::Transport(TransportErrorKind::Custom(Box::new(MockTimeout)));
        assert!(policy.should_retry(&error));
    }

    #[test]
    fn retries_on_http_408() {
        assert!(CustomRetryPolicy.should_retry(&http_error(408)));
    }

    #[test]
    fn retries_on_http_500() {
        assert!(CustomRetryPolicy.should_retry(&http_error(500)));
    }

    #[test]
    fn retries_on_http_502() {
        assert!(CustomRetryPolicy.should_retry(&http_error(502)));
    }

    #[test]
    fn retries_on_http_504() {
        assert!(CustomRetryPolicy.should_retry(&http_error(504)));
    }

    #[test]
    fn does_not_retry_http_404() {
        assert!(!CustomRetryPolicy.should_retry(&http_error(404)));
    }

    #[test]
    fn retries_on_jsonrpc_minus_32601() {
        assert!(CustomRetryPolicy.should_retry(&jsonrpc_error(-32601)));
    }

    #[test]
    fn retries_on_jsonrpc_minus_32603() {
        assert!(CustomRetryPolicy.should_retry(&jsonrpc_error(-32603)));
    }

    #[test]
    fn does_not_retry_jsonrpc_minus_32600() {
        assert!(!CustomRetryPolicy.should_retry(&jsonrpc_error(-32600)));
    }
}
