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

use std::time::{Duration, Instant};

/// UDS diagnostic sessions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    Default = 0x01,
    Programming = 0x02,
    Extended = 0x03,
}

impl Session {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Default),
            0x02 => Some(Self::Programming),
            0x03 => Some(Self::Extended),
            _ => None,
        }
    }

    pub fn byte(self) -> u8 {
        self as u8
    }
}

/// Session state machine with S3 timer
pub struct SessionState {
    current: Session,
    allowed: &'static [Session],
    s3_timeout: Duration,
    last_activity: Instant,
}

impl SessionState {
    pub fn new(allowed: &'static [Session], s3_timeout: Duration) -> Self {
        Self {
            current: Session::Default,
            allowed,
            s3_timeout,
            last_activity: Instant::now(),
        }
    }

    pub fn current(&self) -> Session {
        self.current
    }

    pub fn is_allowed(&self, session: Session) -> bool {
        self.allowed.contains(&session)
    }

    pub fn transition(&mut self, session: Session) -> bool {
        if self.is_allowed(session) {
            self.current = session;
            self.touch();
            true
        } else {
            false
        }
    }

    /// Reset to default session if S3 timer expired
    pub fn tick(&mut self) {
        if self.current != Session::Default && self.last_activity.elapsed() > self.s3_timeout {
            tracing::info!("S3 timer expired, returning to default session");
            self.current = Session::Default;
        }
    }

    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }
}
