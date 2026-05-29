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

use async_trait::async_trait;
use risc0_zkvm::Receipt;
use tokio::sync::oneshot;

use crate::protocol::{RemoteProverError, Request, Response, Risc0Request, Risc0Response};
use crate::rpc::{rpc_system, RpcSender};

use boundless_market::prover_utils::prover::{ProofResult, Prover, ProverError};

type Result<T> = std::result::Result<T, ProverError>;

macro_rules! extract {
    ($variant:ident) => {
        |response| match response {
            Response::Risc0(Risc0Response::$variant(r)) => r.map_err(remote_err),
            _ => Err(ProverError::ProverInternalError("unexpected RPC response".into())),
        }
    };
}

pub struct MultiZkvmClient {
    rpc_sender: RpcSender<tokio::net::tcp::OwnedWriteHalf>,
}

fn remote_err(e: RemoteProverError) -> ProverError {
    ProverError::ProverInternalError(e.0)
}

async fn connect_endpoint(endpoint: &str) -> Result<tokio::net::TcpStream> {
    tokio::net::TcpStream::connect(endpoint)
        .await
        .map_err(|e| ProverError::ProverInternalError(format!("failed to connect to {endpoint}: {e}")))
}

impl MultiZkvmClient {
    pub async fn new(endpoint: &str) -> Result<Self> {
        let tcp_stream = connect_endpoint(endpoint).await?;
        let (rpc_sender, mut rpc_receiver) = rpc_system(tcp_stream);

        tokio::task::spawn(async move {
            rpc_receiver
                .receive_many(|_: Request, _| async move {
                    tracing::error!("unsolicited RPC message");
                })
                .await;
        });

        Ok(Self { rpc_sender })
    }

    async fn send_request<T: Send + 'static>(
        &self,
        request: Request,
        extract: impl FnOnce(Response) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let (tx, rx) = oneshot::channel();
        self.rpc_sender
            .ask(&request, async move |response: Response| {
                let _ = tx.send(extract(response));
            })
            .await
            .map_err(ProverError::UnexpectedError)?;
        rx.await
            .map_err(|_| ProverError::ProverInternalError("RPC response channel closed".into()))?
    }
}

#[async_trait]
impl Prover for MultiZkvmClient {
    async fn has_image(&self, image_id: &str) -> Result<bool> {
        self.send_request(
            Request::Risc0(Risc0Request::HasImage { image_id: image_id.to_string() }),
            extract!(HasImage),
        )
        .await
    }

    async fn upload_input(&self, input: Vec<u8>) -> Result<String> {
        self.send_request(Request::Risc0(Risc0Request::UploadInput { input }), extract!(UploadInput))
            .await
    }

    async fn upload_image(&self, image_id: &str, image: Vec<u8>) -> Result<()> {
        self.send_request(
            Request::Risc0(Risc0Request::UploadImage { image_id: image_id.to_string(), image }),
            extract!(UploadImage),
        )
        .await
    }

    async fn preflight(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
        executor_limit: Option<u64>,
        order_id: &str,
    ) -> Result<ProofResult> {
        self.send_request(
            Request::Risc0(Risc0Request::Preflight {
                image_id: image_id.to_string(),
                input_id: input_id.to_string(),
                assumptions,
                executor_limit,
                order_id: order_id.to_string(),
            }),
            extract!(Preflight),
        )
        .await
    }

    async fn prove_stark(
        &self,
        image_id: &str,
        input_id: &str,
        assumptions: Vec<String>,
    ) -> Result<String> {
        self.send_request(
            Request::Risc0(Risc0Request::ProveStark {
                image_id: image_id.to_string(),
                input_id: input_id.to_string(),
                assumptions,
            }),
            extract!(ProveStark),
        )
        .await
    }

    async fn wait_for_stark(&self, proof_id: &str) -> Result<ProofResult> {
        self.send_request(
            Request::Risc0(Risc0Request::WaitForStark { proof_id: proof_id.to_string() }),
            extract!(WaitForStark),
        )
        .await
    }

    async fn cancel_stark(&self, proof_id: &str) -> Result<()> {
        self.send_request(
            Request::Risc0(Risc0Request::CancelStark { proof_id: proof_id.to_string() }),
            extract!(CancelStark),
        )
        .await
    }

    async fn get_receipt(&self, proof_id: &str) -> Result<Option<Receipt>> {
        self.send_request(
            Request::Risc0(Risc0Request::GetReceipt { proof_id: proof_id.to_string() }),
            extract!(GetReceipt),
        )
        .await
    }

    async fn get_preflight_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>> {
        self.send_request(
            Request::Risc0(Risc0Request::GetPreflightJournal { proof_id: proof_id.to_string() }),
            extract!(GetPreflightJournal),
        )
        .await
    }

    async fn get_journal(&self, proof_id: &str) -> Result<Option<Vec<u8>>> {
        self.send_request(
            Request::Risc0(Risc0Request::GetJournal { proof_id: proof_id.to_string() }),
            extract!(GetJournal),
        )
        .await
    }

    async fn compress(&self, proof_id: &str) -> Result<String> {
        self.send_request(
            Request::Risc0(Risc0Request::Compress { proof_id: proof_id.to_string() }),
            extract!(Compress),
        )
        .await
    }

    async fn get_compressed_receipt(&self, proof_id: &str) -> Result<Option<Vec<u8>>> {
        self.send_request(
            Request::Risc0(Risc0Request::GetCompressedReceipt { proof_id: proof_id.to_string() }),
            extract!(GetCompressedReceipt),
        )
        .await
    }

    async fn compress_blake3_groth16(&self, proof_id: &str) -> Result<String> {
        self.send_request(
            Request::Risc0(Risc0Request::CompressBlake3Groth16 { proof_id: proof_id.to_string() }),
            extract!(CompressBlake3Groth16),
        )
        .await
    }

    async fn get_blake3_groth16_receipt(&self, proof_id: &str) -> Result<Option<Vec<u8>>> {
        self.send_request(
            Request::Risc0(Risc0Request::GetBlake3Groth16Receipt {
                proof_id: proof_id.to_string(),
            }),
            extract!(GetBlake3Groth16Receipt),
        )
        .await
    }
}
