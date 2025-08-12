// Copyright 2025 RISC Zero, Inc.
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

//! The Boundless CLI is a command-line interface for interacting with Boundless.

/// Type used in the [Client] and [StandardRequestBuilder] to indicate that the component in question is not provided.
///
/// Note that this in an [uninhabited type] and cannot be instantiated. When used as
/// `Option<NotProvided>`, the only possible variant for this option is `None`.
///
/// [uninhabited type]: https://smallcultfollowing.com/babysteps/blog/2018/08/13/never-patterns-exhaustive-matching-and-uninhabited-types-oh-my/
/// [StandardRequestBuilder]: crate::request_builder::StandardRequestBuilder
/// [Client]: crate::client::Client
#[derive(Copy, Clone, Debug)]
pub enum NotProvided {}
