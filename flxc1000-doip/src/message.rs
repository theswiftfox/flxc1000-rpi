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

use crate::header::{DoipHeader, PayloadType};

/// Vehicle Announcement / Identification Response Message
#[derive(Debug, Clone)]
pub struct VehicleAnnouncement {
    pub vin: [u8; 17],
    pub logical_address: u16,
    pub eid: [u8; 6],       // entity ID (MAC address)
    pub gid: [u8; 6],       // group ID
    pub further_action: u8, // 0x00 = no further action required
    pub sync_status: u8,    // 0x00 = synchronized
}

impl VehicleAnnouncement {
    pub fn encode(&self) -> Vec<u8> {
        let payload_len = 17 + 2 + 6 + 6 + 1 + 1; // 33 bytes
        let header = DoipHeader::new(PayloadType::VehicleAnnouncementMessage, payload_len as u32);

        let mut buf = Vec::with_capacity(DoipHeader::SIZE + payload_len);
        buf.extend_from_slice(&header.encode());
        buf.extend_from_slice(&self.vin);
        buf.extend_from_slice(&self.logical_address.to_be_bytes());
        buf.extend_from_slice(&self.eid);
        buf.extend_from_slice(&self.gid);
        buf.push(self.further_action);
        buf.push(self.sync_status);
        buf
    }
}

/// Routing Activation Request (received from tester)
#[derive(Debug, Clone)]
pub struct RoutingActivationRequest {
    pub source_address: u16,
    pub activation_type: u8,
    pub reserved: u32,
}

impl RoutingActivationRequest {
    pub fn parse(payload: &[u8]) -> Option<Self> {
        if payload.len() < 7 {
            return None;
        }
        Some(Self {
            source_address: u16::from_be_bytes([payload[0], payload[1]]),
            activation_type: payload[2],
            reserved: u32::from_be_bytes([payload[3], payload[4], payload[5], payload[6]]),
        })
    }
}

/// Routing Activation Response (sent to tester)
#[derive(Debug, Clone)]
pub struct RoutingActivationResponse {
    pub tester_address: u16,
    pub entity_address: u16,
    pub response_code: u8,
    pub reserved: u32,
}

/// Routing activation response codes
pub mod routing_response_code {
    pub const SUCCESS: u8 = 0x10;
    pub const DENIED_UNKNOWN_SA: u8 = 0x00;
    pub const DENIED_ALL_SOCKETS_ACTIVE: u8 = 0x01;
    pub const DENIED_SA_DIFFERENT: u8 = 0x02;
    pub const DENIED_SA_ALREADY_ACTIVE: u8 = 0x03;
}

impl RoutingActivationResponse {
    pub fn encode(&self) -> Vec<u8> {
        let payload_len = 2 + 2 + 1 + 4; // 9 bytes
        let header = DoipHeader::new(PayloadType::RoutingActivationResponse, payload_len as u32);

        let mut buf = Vec::with_capacity(DoipHeader::SIZE + payload_len);
        buf.extend_from_slice(&header.encode());
        buf.extend_from_slice(&self.tester_address.to_be_bytes());
        buf.extend_from_slice(&self.entity_address.to_be_bytes());
        buf.push(self.response_code);
        buf.extend_from_slice(&self.reserved.to_be_bytes());
        buf
    }
}

/// Diagnostic Message (received from tester or sent as response)
#[derive(Debug, Clone)]
pub struct DiagnosticMessage {
    pub source_address: u16,
    pub target_address: u16,
    pub user_data: Vec<u8>,
}

impl DiagnosticMessage {
    pub fn parse(payload: &[u8]) -> Option<Self> {
        if payload.len() < 4 {
            return None;
        }
        Some(Self {
            source_address: u16::from_be_bytes([payload[0], payload[1]]),
            target_address: u16::from_be_bytes([payload[2], payload[3]]),
            user_data: payload[4..].to_vec(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let payload_len = 4 + self.user_data.len();
        let header = DoipHeader::new(PayloadType::DiagnosticMessage, payload_len as u32);

        let mut buf = Vec::with_capacity(DoipHeader::SIZE + payload_len);
        buf.extend_from_slice(&header.encode());
        buf.extend_from_slice(&self.source_address.to_be_bytes());
        buf.extend_from_slice(&self.target_address.to_be_bytes());
        buf.extend_from_slice(&self.user_data);
        buf
    }
}

/// Diagnostic Message Positive Acknowledgement
#[derive(Debug, Clone)]
pub struct DiagnosticMessageAck {
    pub source_address: u16,
    pub target_address: u16,
    pub ack_code: u8,
}

impl DiagnosticMessageAck {
    pub fn encode(&self) -> Vec<u8> {
        let payload_len = 5;
        let header = DoipHeader::new(
            PayloadType::DiagnosticMessagePositiveAck,
            payload_len as u32,
        );

        let mut buf = Vec::with_capacity(DoipHeader::SIZE + payload_len);
        buf.extend_from_slice(&header.encode());
        buf.extend_from_slice(&self.source_address.to_be_bytes());
        buf.extend_from_slice(&self.target_address.to_be_bytes());
        buf.push(self.ack_code);
        buf
    }
}

/// Diagnostic Message Negative Acknowledgement
#[derive(Debug, Clone)]
pub struct DiagnosticMessageNack {
    pub source_address: u16,
    pub target_address: u16,
    pub nack_code: u8,
}

/// NACK codes
pub mod diag_nack_code {
    pub const INVALID_SOURCE_ADDRESS: u8 = 0x02;
    pub const UNKNOWN_TARGET_ADDRESS: u8 = 0x03;
    pub const MESSAGE_TOO_LARGE: u8 = 0x04;
    pub const OUT_OF_MEMORY: u8 = 0x05;
    pub const TARGET_UNREACHABLE: u8 = 0x06;
}

impl DiagnosticMessageNack {
    pub fn encode(&self) -> Vec<u8> {
        let payload_len = 5;
        let header = DoipHeader::new(
            PayloadType::DiagnosticMessageNegativeAck,
            payload_len as u32,
        );

        let mut buf = Vec::with_capacity(DoipHeader::SIZE + payload_len);
        buf.extend_from_slice(&header.encode());
        buf.extend_from_slice(&self.source_address.to_be_bytes());
        buf.extend_from_slice(&self.target_address.to_be_bytes());
        buf.push(self.nack_code);
        buf
    }
}

/// Alive Check Response
pub struct AliveCheckResponse {
    pub source_address: u16,
}

impl AliveCheckResponse {
    pub fn encode(&self) -> Vec<u8> {
        let payload_len = 2;
        let header = DoipHeader::new(PayloadType::AliveCheckResponse, payload_len as u32);

        let mut buf = Vec::with_capacity(DoipHeader::SIZE + payload_len);
        buf.extend_from_slice(&header.encode());
        buf.extend_from_slice(&self.source_address.to_be_bytes());
        buf
    }
}
