// Copyright (c) 2025 RISC Zero, Inc.
//
// All rights reserved.

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
use boundless_market::{
    contracts::IBoundlessMarket,
    order_stream_client::{AuthMsg, ErrMsg, ORDER_WS_PATH},
};
use futures_util::{SinkExt, StreamExt};
use rand::{seq::SliceRandom, Rng};
use std::collections::{hash_map::Entry, HashMap};
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

#[utoipa::path(
    get,
    path = ORDER_WS_PATH,
    params(
        (
            "X-Auth-Data" = AuthMsg, 
            description = "SIWE authentication message (AuthMsg) as a JSON object"
        )
    ),
    responses(
        (status = 200, description = "Websocket upgrade body", body = ()),
        (status = 500, description = "Internal error", body = ErrMsg)
    )
)]
/// Websocket connection point
pub(crate) async fn websocket_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<Arc<AppState>>,
) -> Result<Response, AppError> {
    let auth_header = match headers.get("X-Auth-Data") {
        Some(value) => value,
        None => {
            tracing::warn!("request missing auth header");
            return Ok((StatusCode::BAD_REQUEST, "Missing auth header").into_response());
        }
    };

    // Decode and parse the JSON header into `AuthMsg`
    let auth_msg: AuthMsg = match parse_auth_msg(auth_header) {
        Ok(auth_msg) => auth_msg,
        Err(err) => {
            tracing::error!("Invalid auth-msg format: {err:?}");
            return Ok((StatusCode::BAD_REQUEST, "Invalid auth message format").into_response());
        }
    };

    let client_addr = auth_msg.address();
    let addr_nonce = match state.db.get_nonce(client_addr).await {
        Ok(res) => res,
        Err(OrderDbErr::AddrNotFound(_)) => {
            tracing::warn!("Failed to authorize {client_addr}");
            return Ok((StatusCode::UNAUTHORIZED, "Unauthorized").into_response());
        }
        Err(err) => {
            tracing::warn!("getting DB nonce failed: {client_addr} {err:?}");
            return Err(AppError::InternalErr(err.into()));
        }
    };

    // Check the signature
    if let Err(err) = auth_msg.verify(&state.config.domain, &addr_nonce).await {
        tracing::warn!("Auth message failed to verify: {err:?}");
        return Ok(
            (StatusCode::UNAUTHORIZED, format!("Authentication error: {:?}", err)).into_response()
        );
    }

    // Rotate the customer nonce
    state.db.set_nonce(client_addr).await.context("Failed to update customer nonce")?;

    // Check if the address is already connected
    {
        let connections = state.connections.read().await;
        if connections.contains_key(&client_addr) {
            return Ok((StatusCode::CONFLICT, "Max connections hit (1)").into_response());
        }
        if connections.len() >= state.config.max_connections {
            return Ok((StatusCode::SERVICE_UNAVAILABLE, "Server at capacity").into_response());
        }
    }

    // Connection does not exist, add to pending connections.
    // Note: This is done without holding the lock to state.connections to minimize lock
    // contention. At worst, the server will upgrade the connection and immediately drop it.
    if !state.set_pending_connection(client_addr).await {
        // If the connection is already pending, return an error as max connections is 1.
        return Ok((StatusCode::CONFLICT, "Connection in progress").into_response());
    }

    // Check the balance
    // TODO: This check has several issues:
    // - The balance could change between the check and the connection lifetime
    // - It opens up to an unbounded number of RPC requests to the Ethereum node
    // As such, a more robust solution would be to use a separate task that keeps track of the balances
    // by subscribing to events from the BoundlessMarket contract. Then, the WebSocket connection would be allowed
    // if the balance is above the threshold and the connection would be dropped if the balance falls below the threshold.

    // Skip balance checks if the client_address is on a allow list
    if !state.config.bypass_addrs.contains(&client_addr) {
        let boundless_market =
            IBoundlessMarket::new(state.config.market_address, state.rpc_provider.clone());
        let balance = match boundless_market.balanceOfStake(client_addr).call().await {
            Ok(balance) => balance._0,
            Err(err) => {
                tracing::warn!("Failed to get stake balance for {client_addr}: {err}");
                // Clean up pending connection
                state.remove_pending_connection(&client_addr).await;
                return Ok((StatusCode::INTERNAL_SERVER_ERROR, "Failed to check stake balance")
                    .into_response());
            }
        };
        if balance < state.config.min_balance {
            tracing::warn!("Insufficient stake balance for addr: {client_addr}");
            return Ok((
                StatusCode::UNAUTHORIZED,
                format!("Insufficient stake balance: {} < {}", balance, state.config.min_balance),
            )
                .into_response());
        }
    } else {
        tracing::info!("address: {client_addr} in bypass list, skipping balance checks");
    }

    // Proceed with WebSocket upgrade
    tracing::info!("New webSocket connection from {client_addr}");
    Ok(ws
        .on_failed_upgrade(move |error| {
            tracing::warn!("Failed to upgrade connection for {client_addr}: {error:?}");
        })
        .on_upgrade(move |socket| websocket_connection(socket, client_addr, state)))
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
        let connections = state.connections.read().await;
        let mut connections_list: Vec<_> =
            connections.iter().map(|(addr, conn)| (*addr, conn.sender.clone())).collect();
        connections_list.shuffle(&mut rand::rng());
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
                clients_to_remove.push(address);
            }
        }
    }
    // Remove the clients that have closed their connections
    if !clients_to_remove.is_empty() {
        {
            let mut connections = state.connections.write().await;
            for address in clients_to_remove {
                connections.remove(&address);
            }
        }
    }

    tracing::debug!("Order 0x{:x} broadcasted", db_order.order.request.id);
}

async fn websocket_connection(socket: WebSocket, address: Address, state: Arc<AppState>) {
    let parent_state = state.clone();
    parent_state.ws_tasks.spawn(async move {
        let (mut sender_ws, mut recver_ws) = socket.split();

        let (sender_channel, mut receiver_channel) = mpsc::channel::<String>(state.config.queue_size);

        let is_connected;
        // Add sender to the list of connections
        {
            let mut connections = state.connections.write().await;
            match connections.entry(address) {
                Entry::Occupied(_) => {
                    is_connected = true;
                    tracing::warn!("Client {address} already connected");
                },
                Entry::Vacant(entry) => {
                    is_connected = false;
                    entry.insert(ClientConnection { sender: sender_channel.clone()});
                },
            }
        }

        // Clean up the pending connection entry before upgrading
        state.remove_pending_connection(&address).await;

        if is_connected {
            // Address is already connected, drop additional connection.
            return;
        }

        let mut errors_counter = 0usize;

        let mut ping_data: Option<Vec<u8>> = None;
        let mut ping_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(state.config.ping_time));

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
                                    tracing::warn!("Failed to send message to client {address}: {err}");
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
                _ = ping_interval.tick() => {
                    if ping_data.is_some() {
                        tracing::error!("Client {address} never responded to ping, closing conn");
                        break;
                    }
                    // Send ping
                    let random_bytes: Vec<u8> = rand::rng().random::<[u8; 16]>().into();
                    if let Err(err) = sender_ws.send(Message::Ping(random_bytes.clone())).await {
                        tracing::warn!("Failed to send Ping to {address}: {err:?}");
                        break;
                    }
                    tracing::trace!("Send Ping: {address}");
                    ping_data = Some(random_bytes);
                }
                ws_msg = recver_ws.next() => {
                    // This polls on the recv side of the websocket connection, once a connection closes
                    // either via Err or graceful Message::Close, the next() will return None and we can close the
                    // connection.
                    match ws_msg {
                        Some(Ok(Message::Pong(data))) => {
                            tracing::trace!("Got Pong: {address}");
                            if let Some(send_data) = ping_data.as_ref() {
                                if *send_data != data {
                                    tracing::error!("Invalid ping data from client {address}, closing conn");
                                    break;
                                }
                                ping_data = None;
                                if let Err(err) = state.db.broker_update(address).await {
                                    tracing::error!("Failed to update broker timestamp: {err:?}");
                                    break;
                                }
                            } else {
                                tracing::warn!("Client {address} send out of order pong, closing conn");
                                break;
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            tracing::warn!("Client sent close message, closing conn");
                            break;
                            // TODO: cleaner management of Some(Ok(Message::Close))
                        }
                        Some(Ok(msg)) => {
                            tracing::warn!("Received unexpected message from {address}: {msg:?}");
                        }
                        Some(Err(err)) => {
                            tracing::warn!("Error receiving message from {address}: {err:?}");
                        }
                        None => {
                            tracing::debug!("Empty recv from {address}, closing connections");
                            break;
                        }
                    }
                }
                _ = state.shutdown.cancelled() => {
                    break;
                }
            }
        }
        // Remove the connection when the send loop exits
        state.remove_connection(&address).await;
        tracing::debug!("WebSocket connection closed: {}", address);
    });
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
