// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

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
                let msg = err.to_string();
                msg.contains("os error 104")
            }
            _ => false,
        };
        should_retry || RateLimitRetryPolicy::default().should_retry(error)
    }

    fn backoff_hint(&self, error: &TransportError) -> Option<Duration> {
        RateLimitRetryPolicy::default().backoff_hint(error)
    }
}
