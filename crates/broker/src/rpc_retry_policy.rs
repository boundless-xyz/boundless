// Copyright (c) 2025 RISC Zero Inc,
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use alloy::transports::{
    layers::{RateLimitRetryPolicy, RetryPolicy},
    TransportError, TransportErrorKind,
};
use std::time::Duration;

#[derive(Debug, Copy, Clone, Default)]
pub struct CustomRetryPolicy;

/// The retry policy for the RPC provider used throughout
///
/// This 'extends' the default retry policy to include a retry for
/// OS error 104 which is believed to be behind a number of issues
/// https://github.com/boundless-xyz/boundless/issues/240
impl RetryPolicy for CustomRetryPolicy {
    fn should_retry(&self, error: &TransportError) -> bool {
        let should_retry = match error {
            TransportError::Transport(TransportErrorKind::Custom(err)) => {
                // easier to match against the debug format string because this is what we see in the logs
                let err_debug_str = format!("{:?}", err);
                err_debug_str.contains("os error 104") || err_debug_str.contains("reset by peer")
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
    use alloy::transports::{layers::RetryPolicy, RpcError, TransportErrorKind};
    use std::fmt;

    struct MockError;

    impl fmt::Debug for MockError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("reqwest::Error { kind: Request, source: hyper_util::client::legacy::Error(SendRequest, hyper::Error(Io, Os { code: 104, kind: ConnectionReset, message: \"Connection reset by peer\" })) }")
        }
    }

    impl fmt::Display for MockError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("Mock Error")
        }
    }

    impl std::error::Error for MockError {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            None
        }
    }

    #[test]
    fn retries_on_os_error_104() {
        let policy = CustomRetryPolicy;
        let error = RpcError::Transport(TransportErrorKind::Custom(Box::new(MockError)));
        assert!(policy.should_retry(&error));
    }
}
