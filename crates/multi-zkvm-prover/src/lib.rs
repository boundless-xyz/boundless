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

use boundless_market::prover_utils::prover::{ProverError, ProverObj};
use multi_zkvm_types::protocol::{
    RemoteProverError, Request, Response, Risc0Request, Risc0Response,
};
use multi_zkvm_types::rpc::{rpc_system, RpcMessageId};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};

fn into_remote_err(e: ProverError) -> RemoteProverError {
    RemoteProverError(e.to_string())
}

macro_rules! dispatch {
    ($variant:ident, $expr:expr) => {
        Risc0Response::$variant($expr.await.map_err(into_remote_err))
    };
}

pub struct MultiZkvmServer {
    listener: TcpListener,
    prover: ProverObj,
}

impl MultiZkvmServer {
    pub async fn new(addr: impl ToSocketAddrs, prover: ProverObj) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener, prover })
    }

    pub async fn run(self) {
        loop {
            match self.listener.accept().await {
                Ok((stream, peer_addr)) => {
                    let prover = self.prover.clone();
                    tokio::spawn(async move {
                        tracing::debug!("accepted connection from {peer_addr}");
                        handle_connection(stream, prover).await;
                    });
                }
                Err(e) => {
                    tracing::error!("failed to accept connection: {e}");
                    break;
                }
            }
        }
    }
}

async fn handle_connection(stream: TcpStream, prover: ProverObj) {
    let (rpc_sender, mut rpc_receiver) = rpc_system(stream);

    rpc_receiver
        .receive_many(|request: Request, message_id: Option<RpcMessageId>| {
            let prover = prover.clone();
            let rpc_sender = rpc_sender.clone();
            async move {
                let Some(message_id) = message_id else {
                    tracing::error!("received RPC tell, ignoring");
                    return;
                };
                let response = handle_request(request, &prover).await;
                if let Err(e) = rpc_sender.respond(&response, message_id).await {
                    tracing::error!("failed to send RPC response: {e}");
                }
            }
        })
        .await;
}

async fn handle_request(request: Request, prover: &ProverObj) -> Response {
    match request {
        Request::Risc0(r) => Response::Risc0(handle_risc0_request(r, prover).await),
    }
}

async fn handle_risc0_request(request: Risc0Request, prover: &ProverObj) -> Risc0Response {
    match request {
        Risc0Request::HasImage { image_id } => dispatch!(HasImage, prover.has_image(&image_id)),
        Risc0Request::UploadInput { input } => dispatch!(UploadInput, prover.upload_input(input)),
        Risc0Request::UploadImage { image_id, image } => {
            dispatch!(UploadImage, prover.upload_image(&image_id, image))
        }
        Risc0Request::Preflight { image_id, input_id, assumptions, executor_limit, order_id } => {
            dispatch!(
                Preflight,
                prover.preflight(&image_id, &input_id, assumptions, executor_limit, &order_id)
            )
        }
        Risc0Request::ProveStark { image_id, input_id, assumptions } => {
            dispatch!(ProveStark, prover.prove_stark(&image_id, &input_id, assumptions))
        }
        Risc0Request::WaitForStark { proof_id } => {
            dispatch!(WaitForStark, prover.wait_for_stark(&proof_id))
        }
        Risc0Request::CancelStark { proof_id } => {
            dispatch!(CancelStark, prover.cancel_stark(&proof_id))
        }
        Risc0Request::GetReceipt { proof_id } => {
            dispatch!(GetReceipt, prover.get_receipt(&proof_id))
        }
        Risc0Request::GetPreflightJournal { proof_id } => {
            dispatch!(GetPreflightJournal, prover.get_preflight_journal(&proof_id))
        }
        Risc0Request::GetJournal { proof_id } => {
            dispatch!(GetJournal, prover.get_journal(&proof_id))
        }
        Risc0Request::Compress { proof_id } => dispatch!(Compress, prover.compress(&proof_id)),
        Risc0Request::GetCompressedReceipt { proof_id } => {
            dispatch!(GetCompressedReceipt, prover.get_compressed_receipt(&proof_id))
        }
        Risc0Request::CompressBlake3Groth16 { proof_id } => {
            dispatch!(CompressBlake3Groth16, prover.compress_blake3_groth16(&proof_id))
        }
        Risc0Request::GetBlake3Groth16Receipt { proof_id } => {
            dispatch!(GetBlake3Groth16Receipt, prover.get_blake3_groth16_receipt(&proof_id))
        }
    }
}
