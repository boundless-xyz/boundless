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

//! Integration tests for market indexer

// Common test utilities
#[path = "market/common.rs"]
mod common;

// Test modules
#[path = "market/basic.rs"]
mod basic;

#[path = "market/basic_backfill.rs"]
mod basic_backfill;

#[path = "market/execution.rs"]
mod execution;
