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

use boundless_market::prover_utils::prover::ProofResult;
use risc0_zkvm::Receipt;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct RemoteProverError(pub String);

type Result<T> = std::result::Result<T, RemoteProverError>;

#[derive(Serialize, Deserialize)]
pub enum Request {
    Risc0(Risc0Request),
}

#[derive(Serialize, Deserialize)]
pub enum Risc0Request {
    HasImage {
        image_id: String,
    },
    UploadInput {
        input: Vec<u8>,
    },
    UploadImage {
        image_id: String,
        image: Vec<u8>,
    },
    Preflight {
        image_id: String,
        input_id: String,
        assumptions: Vec<String>,
        executor_limit: Option<u64>,
        order_id: String,
    },
    ProveStark {
        image_id: String,
        input_id: String,
        assumptions: Vec<String>,
    },
    WaitForStark {
        proof_id: String,
    },
    CancelStark {
        proof_id: String,
    },
    GetReceipt {
        proof_id: String,
    },
    GetPreflightJournal {
        proof_id: String,
    },
    GetJournal {
        proof_id: String,
    },
    Compress {
        proof_id: String,
    },
    GetCompressedReceipt {
        proof_id: String,
    },
    CompressBlake3Groth16 {
        proof_id: String,
    },
    GetBlake3Groth16Receipt {
        proof_id: String,
    },
}

#[derive(Serialize, Deserialize)]
pub enum Response {
    Risc0(Risc0Response),
}

#[derive(Serialize, Deserialize)]
pub enum Risc0Response {
    HasImage(Result<bool>),
    UploadInput(Result<String>),
    UploadImage(Result<()>),
    Preflight(Result<ProofResult>),
    ProveStark(Result<String>),
    WaitForStark(Result<ProofResult>),
    CancelStark(Result<()>),
    GetReceipt(Result<Option<Receipt>>),
    GetPreflightJournal(Result<Option<Vec<u8>>>),
    GetJournal(Result<Option<Vec<u8>>>),
    Compress(Result<String>),
    GetCompressedReceipt(Result<Option<Vec<u8>>>),
    CompressBlake3Groth16(Result<String>),
    GetBlake3Groth16Receipt(Result<Option<Vec<u8>>>),
}
