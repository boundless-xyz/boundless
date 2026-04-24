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

//! In-memory storage for Bonsai-compatible objects.
//!
//! Bonsai's SDK uploads images, inputs, and receipts ahead of time and
//! references them by id when creating a session. We keep everything in
//! process memory — objects are lost when the server restarts, which is fine
//! for development and testing.

use std::sync::Arc;

use bytes::Bytes;
use dashmap::DashMap;

use crate::session::SessionRecord;

/// Shared storage handle. Cheap to clone.
#[derive(Clone, Default)]
pub struct Storage {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    /// image_id (hex string) -> ELF/program bytes.
    images: DashMap<String, Bytes>,
    /// input uuid (hex-ish) -> raw input bytes.
    inputs: DashMap<String, Bytes>,
    /// receipt uuid -> receipt bytes (only used to satisfy the assumption
    /// upload flow — we never actually apply assumptions in execute-only).
    receipts: DashMap<String, Bytes>,
    /// session uuid -> record.
    sessions: DashMap<String, Arc<SessionRecord>>,
}

impl Storage {
    pub fn new() -> Self {
        Self::default()
    }

    // ---- images ----

    pub fn image_exists(&self, id: &str) -> bool {
        self.inner.images.contains_key(id)
    }

    pub fn put_image(&self, id: String, bytes: Bytes) {
        self.inner.images.insert(id, bytes);
    }

    pub fn get_image(&self, id: &str) -> Option<Bytes> {
        self.inner.images.get(id).map(|v| v.clone())
    }

    pub fn delete_image(&self, id: &str) -> bool {
        self.inner.images.remove(id).is_some()
    }

    // ---- inputs ----

    pub fn put_input(&self, id: String, bytes: Bytes) {
        self.inner.inputs.insert(id, bytes);
    }

    pub fn get_input(&self, id: &str) -> Option<Bytes> {
        self.inner.inputs.get(id).map(|v| v.clone())
    }

    pub fn delete_input(&self, id: &str) -> bool {
        self.inner.inputs.remove(id).is_some()
    }

    // ---- receipts (assumptions) ----

    pub fn put_receipt(&self, id: String, bytes: Bytes) {
        self.inner.receipts.insert(id, bytes);
    }

    #[allow(dead_code)]
    pub fn get_receipt(&self, id: &str) -> Option<Bytes> {
        self.inner.receipts.get(id).map(|v| v.clone())
    }

    // ---- sessions ----

    pub fn put_session(&self, session: Arc<SessionRecord>) {
        self.inner.sessions.insert(session.uuid.clone(), session);
    }

    pub fn get_session(&self, id: &str) -> Option<Arc<SessionRecord>> {
        self.inner.sessions.get(id).map(|v| v.clone())
    }
}
