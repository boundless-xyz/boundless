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

use std::sync::Arc;

use crate::{AnyDb, DbError, DbObj};
use sqlx::any::install_default_drivers;
use sqlx::AnyPool;
use tempfile::NamedTempFile;

pub struct TestDb {
    pub db: Arc<AnyDb>,
    pub db_url: String,
    pub pool: AnyPool,
    _temp_file: NamedTempFile,
}

impl TestDb {
    pub async fn new() -> Result<Self, DbError> {
        install_default_drivers();
        let temp_file = NamedTempFile::new().unwrap();
        let db_url = format!("sqlite:{}", temp_file.path().display());

        let pool = AnyPool::connect(&db_url).await?;
        let db = Arc::new(AnyDb::new(&db_url).await?);

        Ok(Self { db, db_url: db_url.clone(), pool, _temp_file: temp_file })
    }

    pub fn get_db(&self) -> DbObj {
        self.db.clone()
    }
}
