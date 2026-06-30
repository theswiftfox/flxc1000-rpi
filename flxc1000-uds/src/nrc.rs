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

/// UDS Negative Response Codes (ISO 14229-1)
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Nrc {
    #[error("serviceNotSupported")]
    ServiceNotSupported = 0x11,
    #[error("subFunctionNotSupported")]
    SubFunctionNotSupported = 0x12,
    #[error("incorrectMessageLengthOrInvalidFormat")]
    IncorrectMessageLengthOrInvalidFormat = 0x13,
    #[error("conditionsNotCorrect")]
    ConditionsNotCorrect = 0x22,
    #[error("requestSequenceError")]
    RequestSequenceError = 0x24,
    #[error("requestOutOfRange")]
    RequestOutOfRange = 0x31,
    #[error("securityAccessDenied")]
    SecurityAccessDenied = 0x33,
    #[error("invalidKey")]
    InvalidKey = 0x35,
    #[error("exceededNumberOfAttempts")]
    ExceededNumberOfAttempts = 0x36,
}

impl Nrc {
    pub fn code(self) -> u8 {
        self as u8
    }

    /// Build a negative response: [0x7F, request_sid, nrc]
    pub fn response(self, request_sid: u8) -> Vec<u8> {
        vec![0x7F, request_sid, self.code()]
    }
}
