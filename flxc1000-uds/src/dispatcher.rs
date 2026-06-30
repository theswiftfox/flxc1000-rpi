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

use crate::nrc::Nrc;
use crate::security::SecurityState;
use crate::session::{Session, SessionState};

/// Trait implemented by boot and app binaries to handle UDS service requests.
pub trait ServiceHandler: Send {
    /// Return list of supported SIDs
    fn supported_sids(&self) -> &[u8];

    /// Return list of sessions in which a given SID is allowed.
    /// Default: all sessions.
    fn allowed_sessions(&self, _sid: u8) -> &[Session] {
        &[Session::Default, Session::Programming, Session::Extended]
    }

    /// Whether a SID requires security unlock (and which level)
    fn required_security_level(&self, _sid: u8, _subfunc: Option<u8>) -> Option<u8> {
        None
    }

    /// Handle a UDS request. `data` is the full request starting with SID.
    /// Returns the positive response payload (without the response SID — that is prepended by the dispatcher).
    fn handle(
        &mut self,
        sid: u8,
        data: &[u8],
        session: &mut SessionState,
        security: &mut SecurityState,
    ) -> Result<Vec<u8>, Nrc>;
}

/// UDS request dispatcher — manages session, security, and routes to ServiceHandler
pub struct UdsDispatcher {
    pub session: SessionState,
    pub security: SecurityState,
}

impl UdsDispatcher {
    pub fn new(allowed_sessions: &'static [Session]) -> Self {
        Self {
            session: SessionState::new(allowed_sessions, std::time::Duration::from_millis(5000)),
            security: SecurityState::new(),
        }
    }

    /// Process a raw UDS request. Returns the response bytes to send back.
    pub fn process(&mut self, request: &[u8], handler: &mut dyn ServiceHandler) -> Vec<u8> {
        self.session.touch();
        self.session.tick();

        if request.is_empty() {
            return Nrc::IncorrectMessageLengthOrInvalidFormat.response(0x00);
        }

        let sid = request[0];

        // Check if SID is supported
        if !handler.supported_sids().contains(&sid) {
            return Nrc::ServiceNotSupported.response(sid);
        }

        // Check session constraints
        let allowed = handler.allowed_sessions(sid);
        if !allowed.contains(&self.session.current()) {
            return Nrc::ConditionsNotCorrect.response(sid);
        }

        // Check security constraints
        let subfunc = request.get(1).copied();
        if let Some(level) = handler.required_security_level(sid, subfunc) {
            if !self.security.is_unlocked(level) {
                return Nrc::SecurityAccessDenied.response(sid);
            }
        }

        // Built-in service handling for 0x10 (DiagnosticSessionControl) and 0x27 (SecurityAccess)
        // and 0x3E (TesterPresent)
        match sid {
            0x10 => return self.handle_dsc(request),
            0x27 => return self.handle_security_access(request),
            0x3E => return self.handle_tester_present(request),
            _ => {}
        }

        // Delegate to handler
        match handler.handle(sid, request, &mut self.session, &mut self.security) {
            Ok(payload) => {
                let mut resp = Vec::with_capacity(1 + payload.len());
                resp.push(sid + 0x40); // positive response SID
                resp.extend_from_slice(&payload);
                resp
            }
            Err(nrc) => nrc.response(sid),
        }
    }

    fn handle_dsc(&mut self, request: &[u8]) -> Vec<u8> {
        let sid = 0x10;
        if request.len() < 2 {
            return Nrc::IncorrectMessageLengthOrInvalidFormat.response(sid);
        }

        let subfunc = request[1] & 0x7F; // mask suppress-positive bit
        let suppress = request[1] & 0x80 != 0;

        let session = match Session::from_byte(subfunc) {
            Some(s) => s,
            None => return Nrc::SubFunctionNotSupported.response(sid),
        };

        if !self.session.is_allowed(session) {
            return Nrc::SubFunctionNotSupported.response(sid);
        }

        // Transition resets security on non-default
        let prev = self.session.current();
        self.session.transition(session);

        // Reset security when switching sessions (except staying in same session)
        if prev != session && session == Session::Default {
            self.security.reset();
        }

        if suppress {
            return Vec::new();
        }

        // Positive response: [0x50, subfunc, P2_hi, P2_lo, P2star_hi, P2star_lo]
        // P2 = 50ms (0x0032), P2* = 5000ms (0x01F4)
        vec![0x50, subfunc, 0x00, 0x32, 0x01, 0xF4]
    }

    fn handle_security_access(&mut self, request: &[u8]) -> Vec<u8> {
        let sid = 0x27;
        if request.len() < 2 {
            return Nrc::IncorrectMessageLengthOrInvalidFormat.response(sid);
        }

        let subfunc = request[1];

        // Seed request (odd subfunctions)
        if subfunc % 2 == 1 {
            match self.security.request_seed(subfunc) {
                Ok(seed) => {
                    let mut resp = vec![0x67, subfunc];
                    resp.extend_from_slice(&seed);
                    resp
                }
                Err(nrc) => nrc.response(sid),
            }
        }
        // Key submission (even subfunctions)
        else {
            let key_bytes = &request[2..];
            match self.security.submit_key(subfunc, key_bytes) {
                Ok(()) => vec![0x67, subfunc],
                Err(nrc) => nrc.response(sid),
            }
        }
    }

    fn handle_tester_present(&mut self, request: &[u8]) -> Vec<u8> {
        let sid = 0x3E;
        if request.len() < 2 {
            return Nrc::IncorrectMessageLengthOrInvalidFormat.response(sid);
        }

        let subfunc = request[1];
        let suppress = subfunc & 0x80 != 0;
        let subfunc_value = subfunc & 0x7F;

        if subfunc_value != 0x00 {
            return Nrc::SubFunctionNotSupported.response(sid);
        }

        self.session.touch();

        if suppress {
            return Vec::new();
        }

        vec![0x7E, 0x00]
    }

    /// Tick the session timer — call periodically
    pub fn tick(&mut self) {
        self.session.tick();
    }
}
