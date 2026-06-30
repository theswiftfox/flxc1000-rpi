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

use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;

use crate::connection::{self, UdsRequest};
use crate::header::{DoipHeader, PayloadType};
use crate::message::VehicleAnnouncement;

/// DoIP server configuration
#[derive(Debug, Clone)]
pub struct DoipConfig {
    pub ecu_address: u16,
    pub vin: [u8; 17],
    pub eid: [u8; 6],
    pub gid: [u8; 6],
}

impl DoipConfig {
    pub fn new(ecu_address: u16, vin: &[u8; 17]) -> Self {
        Self {
            ecu_address,
            vin: *vin,
            eid: [0x00; 6],
            gid: [0x00; 6],
        }
    }

    pub fn with_eid(mut self, eid: [u8; 6]) -> Self {
        self.eid = eid;
        self
    }
}

/// DoIP server — listens on UDP 13400 (vehicle identification) and TCP 13400 (diagnostic)
pub struct DoipServer {
    config: DoipConfig,
}

impl DoipServer {
    pub fn new(config: DoipConfig) -> Self {
        Self { config }
    }

    /// Run the DoIP server. Returns a channel receiver for incoming UDS requests.
    /// Each UDS request contains a response channel to send the reply back.
    pub async fn run(self) -> Result<mpsc::Receiver<UdsRequest>, Box<dyn std::error::Error>> {
        let (uds_tx, uds_rx) = mpsc::channel::<UdsRequest>(32);

        let config = self.config.clone();
        tokio::spawn(async move {
            if let Err(e) = run_udp_server(&config).await {
                tracing::error!("UDP server error: {}", e);
            }
        });

        let config = self.config.clone();
        let tx = uds_tx.clone();
        tokio::spawn(async move {
            if let Err(e) = run_tcp_server(&config, tx).await {
                tracing::error!("TCP server error: {}", e);
            }
        });

        Ok(uds_rx)
    }
}

async fn run_udp_server(config: &DoipConfig) -> Result<(), Box<dyn std::error::Error>> {
    let socket = UdpSocket::bind("0.0.0.0:13400").await?;
    socket.set_broadcast(true)?;
    tracing::info!("DoIP UDP server listening on 0.0.0.0:13400");

    let mut buf = [0u8; 1024];

    loop {
        let (n, peer) = socket.recv_from(&mut buf).await?;

        if n < DoipHeader::SIZE {
            continue;
        }

        let header = match DoipHeader::parse(&buf[..n]) {
            Some(h) => h,
            None => continue,
        };

        match PayloadType::from_u16(header.payload_type) {
            Some(PayloadType::VehicleIdentificationRequest)
            | Some(PayloadType::VehicleIdentificationRequestEid)
            | Some(PayloadType::VehicleIdentificationRequestVin) => {
                tracing::info!(?peer, "Vehicle identification request received");

                let vam = VehicleAnnouncement {
                    vin: config.vin,
                    logical_address: config.ecu_address,
                    eid: config.eid,
                    gid: config.gid,
                    further_action: 0x00,
                    sync_status: 0x00,
                };

                if let Err(e) = socket.send_to(&vam.encode(), peer).await {
                    tracing::error!("Failed to send VAM: {}", e);
                }
            }
            _ => {
                tracing::debug!(
                    ?peer,
                    payload_type = header.payload_type,
                    "Ignoring UDP message"
                );
            }
        }
    }
}

async fn run_tcp_server(
    config: &DoipConfig,
    uds_tx: mpsc::Sender<UdsRequest>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind("0.0.0.0:13400").await?;
    tracing::info!("DoIP TCP server listening on 0.0.0.0:13400");

    loop {
        let (stream, peer) = listener.accept().await?;
        tracing::info!(?peer, "TCP connection accepted");

        let ecu_address = config.ecu_address;
        let tx = uds_tx.clone();

        tokio::spawn(async move {
            connection::handle_connection(stream, ecu_address, tx).await;
        });
    }
}

/// Try to read the MAC address of the first non-loopback interface for EID.
/// Falls back to zeros on failure.
pub fn get_mac_address() -> [u8; 6] {
    // On Linux, read /sys/class/net/<iface>/address
    if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == "lo" {
                continue;
            }
            let path = format!("/sys/class/net/{}/address", name);
            if let Ok(addr_str) = std::fs::read_to_string(&path) {
                let addr_str = addr_str.trim();
                let parts: Vec<&str> = addr_str.split(':').collect();
                if parts.len() == 6 {
                    let mut mac = [0u8; 6];
                    for (i, part) in parts.iter().enumerate() {
                        if let Ok(b) = u8::from_str_radix(part, 16) {
                            mac[i] = b;
                        }
                    }
                    if mac != [0u8; 6] {
                        return mac;
                    }
                }
            }
        }
    }
    [0u8; 6]
}
