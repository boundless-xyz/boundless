// Copyright (c) 2024 RISC Zero, Inc.
//
// All rights reserved.

#[cfg(feature = "build-guest")]
include!(concat!(env!("OUT_DIR"), "/methods.rs"));

#[cfg(not(feature = "build-guest"))]
mod elfs {
    // NOTE: Image IDs are cast to [u32; 8] to match the auto-generated methods.rs file.

    pub const BOUNDLESS_POVW_MINT_CALCULATOR_ELF: &[u8] =
        include_bytes!("../elfs/boundless-povw-mint-calculator.bin");
    pub const BOUNDLESS_POVW_MINT_CALCULATOR_ID: [u32; 8] =
        bytemuck::must_cast(*include_bytes!("../elfs/boundless-povw-mint-calculator.iid"));
    pub const BOUNDLESS_POVW_LOG_UPDATER_ELF: &[u8] =
        include_bytes!("../elfs/boundless-povw-log-updater.bin");
    pub const BOUNDLESS_POVW_LOG_UPDATER_ID: [u32; 8] =
        bytemuck::must_cast(*include_bytes!("../elfs/boundless-povw-log-updater.iid"));
}
#[cfg(not(feature = "build-guest"))]
pub use elfs::*;

pub mod log_updater;
pub mod mint_calculator;
pub mod zkc;
