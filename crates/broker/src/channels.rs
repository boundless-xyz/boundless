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

//! Channel helpers shared across broker services.
//!
//! Several services (e.g. `OrderEvaluator`, `OrderPricer`, `OrderCommitter`)
//! consume from an `mpsc::Receiver` from inside a `Clone` service struct that
//! is restarted by the supervisor. Cloning a service requires the receiver to
//! be wrapped in `Arc<Mutex<…>>` so the same receiver is shared across clones.
//! The wrapping is mechanical and identical at every call site — this module
//! captures it once.

use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

/// A receiver that can be cheaply cloned and shared across `Clone`d services.
///
/// Use together with [`shared_channel`] when the receiver needs to be owned
/// by a service that the supervisor may clone on restart.
// Unused until services are migrated to consume `SharedReceiver` directly
// (Phase 3 of the broker restructure). Kept here as additive infrastructure.
#[allow(dead_code)]
pub type SharedReceiver<T> = Arc<Mutex<mpsc::Receiver<T>>>;

/// Creates an [`mpsc`] channel whose receiver is wrapped as a [`SharedReceiver`].
///
/// Equivalent to `mpsc::channel(capacity)` followed by manually wrapping the
/// receiver in `Arc<Mutex<…>>` — bundled here so service constructors don't
/// have to repeat the wrapping.
#[allow(dead_code)]
pub fn shared_channel<T>(capacity: usize) -> (mpsc::Sender<T>, SharedReceiver<T>) {
    let (tx, rx) = mpsc::channel(capacity);
    (tx, Arc::new(Mutex::new(rx)))
}
