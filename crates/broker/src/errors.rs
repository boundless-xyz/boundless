// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

pub trait CodedError: std::error::Error {
    fn code(&self) -> &str;
}
