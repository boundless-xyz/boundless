// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

#[cfg(feature = "build-guest")]
include!(concat!(env!("OUT_DIR"), "/methods.rs"));

pub mod log_updater;
pub mod mint_calculator;
