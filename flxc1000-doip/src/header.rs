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

/// DoIP protocol version (ISO 13400-2:2012)
pub const PROTOCOL_VERSION: u8 = 0x02;
pub const INVERSE_VERSION: u8 = !PROTOCOL_VERSION;

/// DoIP payload types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadType {
    VehicleIdentificationRequest = 0x0001,
    VehicleIdentificationRequestEid = 0x0002,
    VehicleIdentificationRequestVin = 0x0003,
    VehicleAnnouncementMessage = 0x0004,
    RoutingActivationRequest = 0x0005,
    RoutingActivationResponse = 0x0006,
    AliveCheckRequest = 0x0007,
    AliveCheckResponse = 0x0008,
    DiagnosticMessage = 0x8001,
    DiagnosticMessagePositiveAck = 0x8002,
    DiagnosticMessageNegativeAck = 0x8003,
}

impl PayloadType {
    pub fn from_u16(v: u16) -> Option<Self> {
        match v {
            0x0001 => Some(Self::VehicleIdentificationRequest),
            0x0002 => Some(Self::VehicleIdentificationRequestEid),
            0x0003 => Some(Self::VehicleIdentificationRequestVin),
            0x0004 => Some(Self::VehicleAnnouncementMessage),
            0x0005 => Some(Self::RoutingActivationRequest),
            0x0006 => Some(Self::RoutingActivationResponse),
            0x0007 => Some(Self::AliveCheckRequest),
            0x0008 => Some(Self::AliveCheckResponse),
            0x8001 => Some(Self::DiagnosticMessage),
            0x8002 => Some(Self::DiagnosticMessagePositiveAck),
            0x8003 => Some(Self::DiagnosticMessageNegativeAck),
            _ => None,
        }
    }

    pub fn as_u16(self) -> u16 {
        self as u16
    }
}

/// DoIP generic header (8 bytes)
#[derive(Debug, Clone)]
pub struct DoipHeader {
    pub protocol_version: u8,
    pub inverse_version: u8,
    pub payload_type: u16,
    pub payload_length: u32,
}

impl DoipHeader {
    pub const SIZE: usize = 8;

    pub fn new(payload_type: PayloadType, payload_length: u32) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            inverse_version: INVERSE_VERSION,
            payload_type: payload_type.as_u16(),
            payload_length,
        }
    }

    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < Self::SIZE {
            return None;
        }
        Some(Self {
            protocol_version: data[0],
            inverse_version: data[1],
            payload_type: u16::from_be_bytes([data[2], data[3]]),
            payload_length: u32::from_be_bytes([data[4], data[5], data[6], data[7]]),
        })
    }

    pub fn encode(&self) -> [u8; 8] {
        let pt = self.payload_type.to_be_bytes();
        let pl = self.payload_length.to_be_bytes();
        [
            self.protocol_version,
            self.inverse_version,
            pt[0],
            pt[1],
            pl[0],
            pl[1],
            pl[2],
            pl[3],
        ]
    }

    pub fn is_valid(&self) -> bool {
        self.protocol_version == PROTOCOL_VERSION && self.inverse_version == INVERSE_VERSION
    }
}
