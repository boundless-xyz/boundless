// Copyright 2025 RISC Zero, Inc.
//
// Use of this source code is governed by the Business Source License
// as found in the LICENSE-BSL file.

use ruint::aliases::U160;
use std::env;

/// POVW (Proof of Verifiable Work) configuration and utilities
#[derive(Clone)]
pub struct PovwConfig {
    /// 160-bit work log identifier (unique per prover)
    pub log_id: U160,
    /// Job number within the work log
    pub job_number: u64,
}

impl PovwConfig {
    /// Create a new POVW configuration
    pub fn new(log_id: U160, job_number: u64) -> Self {
        Self { log_id, job_number }
    }

    /// Create POVW config from environment variables
    /// Only returns Some if POVW_LOG_ID is set
    pub fn from_env() -> Option<Self> {
        let log_id = match env::var("POVW_LOG_ID") {
            Ok(id) => id.parse::<U160>().ok()?,
            Err(_) => return None, // POVW not enabled
        };

        // Generate random job number if not specified
        let job_number = env::var("POVW_JOB_NUMBER")
            .unwrap_or_else(|_| random_job_number().to_string())
            .parse::<u64>()
            .unwrap_or_else(|_| random_job_number());

        Some(Self::new(log_id, job_number))
    }

    /// Convert to tuple format expected by RISC Zero
    pub fn to_tuple(&self) -> (U160, u64) {
        (self.log_id, self.job_number)
    }

    /// Get the log ID as a string
    pub fn log_id_string(&self) -> String {
        format!("0x{:X}", self.log_id)
    }
}

/// Generate a random job number
pub fn random_job_number() -> u64 {
    use rand::Rng;
    let mut rng = rand::rng();
    rng.random()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_povw_config_creation() {
        let log_id = U160::from(0x1234567890abcdefu64);
        let job_number = 42;

        let config = PovwConfig::new(log_id, job_number);
        assert_eq!(config.log_id, log_id);
        assert_eq!(config.job_number, job_number);
    }

    #[test]
    fn test_povw_config_to_tuple() {
        let log_id = U160::from(0x1234567890abcdefu64);
        let job_number = 42;

        let config = PovwConfig::new(log_id, job_number);
        let (tuple_log_id, tuple_job_number) = config.to_tuple();

        assert_eq!(tuple_log_id, log_id);
        assert_eq!(tuple_job_number, job_number);
    }

    #[test]
    fn test_povw_config_log_id_string() {
        let log_id = U160::from(0x1234567890abcdefu64);
        let config = PovwConfig::new(log_id, 42);

        let log_id_string = config.log_id_string();
        assert_eq!(log_id_string, "0x1234567890ABCDEF");
    }

    #[test]
    fn test_povw_config_from_env_disabled() {
        // Clear any existing POVW environment variables
        env::remove_var("POVW_LOG_ID");
        env::remove_var("POVW_JOB_NUMBER");

        let config = PovwConfig::from_env();
        assert!(config.is_none());
    }

    #[test]
    fn test_povw_config_from_env_enabled() {
        // Set POVW environment variables
        env::set_var("POVW_LOG_ID", "0x1234567890abcdef");
        env::set_var("POVW_JOB_NUMBER", "42");

        let config = PovwConfig::from_env();
        assert!(config.is_some());

        if let Some(config) = config {
            assert_eq!(config.log_id, U160::from(0x1234567890abcdefu64));
            assert_eq!(config.job_number, 42);
        }

        // Clean up
        env::remove_var("POVW_LOG_ID");
        env::remove_var("POVW_JOB_NUMBER");
    }

    #[test]
    fn test_povw_config_from_env_without_job_number() {
        // Set only POVW_LOG_ID
        env::set_var("POVW_LOG_ID", "0x1234567890abcdef");
        env::remove_var("POVW_JOB_NUMBER");

        let config = PovwConfig::from_env();
        assert!(config.is_some());

        // Clean up
        env::remove_var("POVW_LOG_ID");
    }
}
