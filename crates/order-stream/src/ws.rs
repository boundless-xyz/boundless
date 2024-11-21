use alloy::primitives::Address;
use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use boundless_market::{contracts::IProofMarket, order_stream_client::AuthMsg};
use futures_util::{SinkExt, StreamExt};
use rand::{seq::SliceRandom, thread_rng};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::{sync::mpsc, task::JoinHandle};

use crate::order_db::{DbOrder, OrderDbErr, OrderStream};
use crate::{AppError, AppState};

pub(crate) struct ClientConnection {
    sender: mpsc::Sender<String>, // Channel to send messages to this client
}

pub(crate) type ConnectionsMap = HashMap<Address, ClientConnection>;

fn parse_auth_msg(value: &HeaderValue) -> Result<AuthMsg> {
    let json_str = value.to_str().context("Invalid header encoding")?;
    serde_json::from_str(json_str).context("Failed to parse JSON")
}

// WebSocket upgrade handler
pub(crate) async fn websocket_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    let auth_header = match headers.get("X-Auth-Data") {
        Some(value) => value,
        None => return Ok((StatusCode::BAD_REQUEST, "Missing auth header").into_response()),
    };

    // Decode and parse the JSON header into `AuthMsg`
    let auth_msg: AuthMsg = match parse_auth_msg(auth_header) {
        Ok(auth_msg) => auth_msg,
        Err(_) => {
            return Ok((StatusCode::BAD_REQUEST, "Invalid auth message format").into_response())
        }
    };

    let client_addr = auth_msg.address();
    let addr_nonce = state.db.get_nonce(client_addr).await.context("Failed to fetch addr nonce")?;

    // Check the signature
    if let Err(err) = auth_msg.verify(&state.config.domain, &addr_nonce).await {
        return Ok(
            (StatusCode::UNAUTHORIZED, format!("Authentication error: {:?}", err)).into_response()
        );
    }

    // Rotate the customer nonce
    state.db.set_nonce(client_addr).await.context("Failed to update customer nonce")?;

    // Check if the address is already connected
    {
        match state.db.connect_broker(client_addr).await {
            Err(OrderDbErr::MaxConnections) => {
                return Ok((StatusCode::CONFLICT, format!("Max connections hit")).into_response())
            }
            Err(err) => return Err(AppError::InternalErr(anyhow::anyhow!(err))),
            _ => {}
        }
        let connections = state.connections.lock().await;
        if connections.len() >= state.config.max_connections {
            return Ok((StatusCode::SERVICE_UNAVAILABLE, "Server at capacity").into_response());
        }
    }

    // Check the balance
    // TODO: This check has several issues:
    // - The balance could change between the check and the connection lifetime
    // - It opens up to an unbounded number of RPC requests to the Ethereum node
    // As such, a more robust solution would be to use a separate task that keeps track of the balances
    // by subscribing to events from the ProofMarket contract. Then, the WebSocket connection would be allowed
    // if the balance is above the threshold and the connection would be dropped if the balance falls below the threshold.

    // Skip balance checks if the client_address is on a allow list
    if !state.config.bypass_addrs.contains(&client_addr) {
        let proof_market =
            IProofMarket::new(state.config.market_address, state.rpc_provider.clone());
        let balance = proof_market.balanceOf(client_addr).call().await.unwrap()._0;
        if balance < state.config.min_balance {
            return Ok((
                StatusCode::UNAUTHORIZED,
                format!("Insufficient balance: {} < {}", balance, state.config.min_balance),
            )
                .into_response());
        }
    } else {
        tracing::info!("address: {client_addr} in bypass list, skipping balance checks");
    }

    // Proceed with WebSocket upgrade
    tracing::debug!("New webSocket connection from {}", client_addr);
    Ok(ws.on_upgrade(move |socket| websocket_connection(socket, client_addr, state)))
}

// Function to broadcast an order to all WebSocket clients in random order
async fn broadcast_order(db_order: &DbOrder, state: Arc<AppState>) {
    let order_json = match serde_json::to_string(&db_order) {
        Ok(order_json) => order_json,
        Err(err) => {
            tracing::error!("Failed to serialize order 0x{:x}: {}", db_order.order.request.id, err);
            return;
        }
    };

    // Shuffle the connections
    let connections_list = {
        let connections = state.connections.lock().await;
        let mut connections_list: Vec<_> =
            connections.iter().map(|(addr, conn)| (addr.clone(), conn.sender.clone())).collect();
        connections_list.shuffle(&mut thread_rng());
        connections_list
    };

    let mut clients_to_remove = Vec::new();
    for (address, sender) in connections_list {
        match sender.try_send(order_json.clone()) {
            Ok(_) => {}
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!("Client {}'s message queue is full, message dropped", address);
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                tracing::warn!("Client {}'s message queue is closed, removing client", address);
                // Add the client to the list of clients to remove
                clients_to_remove.push(address.clone());
            }
        }
    }
    // Remove the clients that have closed their connections
    if !clients_to_remove.is_empty() {
        {
            let mut connections = state.connections.lock().await;
            for address in clients_to_remove {
                connections.remove(&address);
                if let Err(err) = state.db.disconnect_broker(address).await {
                    tracing::error!(
                        "Failed to remove broker connection from DB: {address} - {err:?}"
                    );
                }
            }
        }
    }

    tracing::debug!("Order 0x{:x} broadcasted", db_order.order.request.id);
}

async fn websocket_connection(socket: WebSocket, address: Address, state: Arc<AppState>) {
    let (mut sender_ws, mut recver_ws) = socket.split();

    let (sender_channel, mut receiver_channel) = mpsc::channel::<String>(state.config.queue_size);

    // Add sender to the list of connections
    {
        let mut connections = state.connections.lock().await;
        connections.insert(address, ClientConnection { sender: sender_channel.clone() });
    }

    let mut errors_counter = 0usize;

    loop {
        tokio::select! {
            msg = receiver_channel.recv() => {
                match msg {
                    Some(msg) => {
                        match sender_ws.send(Message::Text(msg)).await {
                            Ok(_) => {
                                // Reset the error counter on successful send
                                errors_counter = 0;
                            }
                            Err(err) => {
                                tracing::warn!("Failed to send message to client {}: {}", address, err);
                                errors_counter += 1;
                                if errors_counter > 10 {
                                    tracing::warn!(
                                        "Too many consecutive send errors to client {}; disconnecting",
                                        address
                                    );
                                    break;
                                }
                            }
                        }
                    }
                    None => break,
                }
            }
            ws_msg = recver_ws.next() => {
                // This polls on the recv side of the websocket connection, once a connection closes
                // either via Err or graceful Message::Close, the nex() will return None and we can close the
                // connection.
                match ws_msg {
                    Some(_) => {
                        // TODO: cleaner management of Some(Ok(Message::Close))
                    }
                    None => {
                        tracing::debug!("Empty recv, closing connections");
                        break;
                    }
                }
            }
        }
    }

    // Remove the connection when the send loop exits
    let mut connections = state.connections.lock().await;
    connections.remove(&address);
    if let Err(err) = state.db.disconnect_broker(address).await {
        tracing::error!("Failed to remove broker connection from DB: {address} - {err:?}");
    }
    tracing::debug!("WebSocket connection closed: {}", address);
}

pub(crate) fn start_broadcast_task(
    app_state: Arc<AppState>,
    mut order_stream: OrderStream,
) -> JoinHandle<Result<(), OrderDbErr>> {
    tokio::spawn(async move {
        while let Some(order) = order_stream.next().await {
            let order = order?;
            broadcast_order(&order, app_state.clone()).await;
        }
        Ok(())
    })
}
