/*
 * SPDX-FileCopyrightText: 2026 Copyright (c) Contributors to the Eclipse Foundation
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Apache License Version 2.0 which is available at
 * https://www.apache.org/licenses/LICENSE-2.0
 *
 * SPDX-License-Identifier: Apache-2.0
 */

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::header::{DoipHeader, PayloadType};
use crate::message::*;

const INACTIVITY_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_PAYLOAD_SIZE: u32 = 64 * 1024; // 64 KiB max payload

/// A request from the DoIP layer to the UDS layer
#[derive(Debug)]
pub struct UdsRequest {
    pub source_address: u16,
    pub target_address: u16,
    pub data: Vec<u8>,
    pub response_tx: mpsc::Sender<Vec<u8>>,
}

/// State of a single TCP connection
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConnectionState {
    AwaitingActivation,
    Active { tester_address: u16 },
}

/// Handle a single DoIP TCP connection
pub async fn handle_connection(
    mut stream: TcpStream,
    ecu_address: u16,
    uds_tx: mpsc::Sender<UdsRequest>,
) {
    let peer = stream.peer_addr().ok();
    tracing::info!(?peer, "DoIP TCP connection accepted");

    let mut state = ConnectionState::AwaitingActivation;
    let mut buf = vec![0u8; 8 + MAX_PAYLOAD_SIZE as usize];
    let mut last_activity = Instant::now();

    loop {
        let timeout = tokio::time::sleep_until(last_activity + INACTIVITY_TIMEOUT);

        tokio::select! {
            _ = timeout => {
                tracing::info!(?peer, "Inactivity timeout, closing connection");
                break;
            }
            result = stream.read(&mut buf) => {
                match result {
                    Ok(0) => {
                        tracing::info!(?peer, "Connection closed by peer");
                        break;
                    }
                    Ok(n) => {
                        last_activity = Instant::now();
                        if let Err(e) = process_tcp_data(
                            &buf[..n],
                            &mut stream,
                            &mut state,
                            ecu_address,
                            &uds_tx,
                        ).await {
                            tracing::error!(?peer, "Error processing TCP data: {}", e);
                            break;
                        }
                    }
                    Err(e) => {
                        tracing::error!(?peer, "Read error: {}", e);
                        break;
                    }
                }
            }
        }
    }
}

async fn process_tcp_data(
    data: &[u8],
    stream: &mut TcpStream,
    state: &mut ConnectionState,
    ecu_address: u16,
    uds_tx: &mpsc::Sender<UdsRequest>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut offset = 0;

    while offset + DoipHeader::SIZE <= data.len() {
        let header = match DoipHeader::parse(&data[offset..]) {
            Some(h) => h,
            None => break,
        };

        let total_len = DoipHeader::SIZE + header.payload_length as usize;
        if offset + total_len > data.len() {
            tracing::warn!("Incomplete DoIP message, waiting for more data");
            break;
        }

        let payload = &data[offset + DoipHeader::SIZE..offset + total_len];

        match PayloadType::from_u16(header.payload_type) {
            Some(PayloadType::RoutingActivationRequest) => {
                handle_routing_activation(payload, stream, state, ecu_address).await?;
            }
            Some(PayloadType::DiagnosticMessage) => {
                handle_diagnostic_message(payload, stream, state, ecu_address, uds_tx).await?;
            }
            Some(PayloadType::AliveCheckRequest) => {
                let resp = AliveCheckResponse {
                    source_address: ecu_address,
                };
                stream.write_all(&resp.encode()).await?;
            }
            other => {
                tracing::warn!(
                    "Unsupported DoIP payload type: {:?} (0x{:04X})",
                    other,
                    header.payload_type
                );
            }
        }

        offset += total_len;
    }

    Ok(())
}

async fn handle_routing_activation(
    payload: &[u8],
    stream: &mut TcpStream,
    state: &mut ConnectionState,
    ecu_address: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    let req =
        RoutingActivationRequest::parse(payload).ok_or("Invalid routing activation request")?;

    tracing::info!(
        tester_address = format_args!("0x{:04X}", req.source_address),
        activation_type = req.activation_type,
        "Routing activation request"
    );

    let resp = RoutingActivationResponse {
        tester_address: req.source_address,
        entity_address: ecu_address,
        response_code: routing_response_code::SUCCESS,
        reserved: 0,
    };

    stream.write_all(&resp.encode()).await?;
    *state = ConnectionState::Active {
        tester_address: req.source_address,
    };
    tracing::info!(
        tester_address = format_args!("0x{:04X}", req.source_address),
        "Routing activated"
    );
    Ok(())
}

async fn handle_diagnostic_message(
    payload: &[u8],
    stream: &mut TcpStream,
    state: &mut ConnectionState,
    ecu_address: u16,
    uds_tx: &mpsc::Sender<UdsRequest>,
) -> Result<(), Box<dyn std::error::Error>> {
    let tester_address = match state {
        ConnectionState::Active { tester_address } => *tester_address,
        ConnectionState::AwaitingActivation => {
            tracing::warn!("Diagnostic message received before routing activation");
            return Ok(());
        }
    };

    let msg = DiagnosticMessage::parse(payload).ok_or("Invalid diagnostic message")?;

    // Check target address
    if msg.target_address != ecu_address && msg.target_address != 0xFFFF {
        let nack = DiagnosticMessageNack {
            source_address: ecu_address,
            target_address: msg.source_address,
            nack_code: diag_nack_code::UNKNOWN_TARGET_ADDRESS,
        };
        stream.write_all(&nack.encode()).await?;
        return Ok(());
    }

    // Send positive ack
    let ack = DiagnosticMessageAck {
        source_address: ecu_address,
        target_address: msg.source_address,
        ack_code: 0x00,
    };
    stream.write_all(&ack.encode()).await?;

    // Forward UDS request and await response
    let (resp_tx, mut resp_rx) = mpsc::channel::<Vec<u8>>(1);
    let uds_req = UdsRequest {
        source_address: msg.source_address,
        target_address: msg.target_address,
        data: msg.user_data,
        response_tx: resp_tx,
    };

    uds_tx
        .send(uds_req)
        .await
        .map_err(|_| "UDS handler closed")?;

    // Wait for UDS response
    if let Some(response_data) = resp_rx.recv().await {
        if !response_data.is_empty() {
            let resp_msg = DiagnosticMessage {
                source_address: ecu_address,
                target_address: tester_address,
                user_data: response_data,
            };
            stream.write_all(&resp_msg.encode()).await?;
        }
    }

    Ok(())
}
