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

use std::time::Instant;

use crate::nrc::Nrc;

const MAX_ATTEMPTS: u8 = 3;
const LOCKOUT_DURATION_MS: u64 = 10_000;

/// Security access state machine
pub struct SecurityState {
    unlocked_levels: u8,             // bitmask of unlocked levels
    pending_seed: Option<(u8, u32)>, // (level, seed)
    failed_attempts: u8,
    lockout_until: Option<Instant>,
}

impl SecurityState {
    pub fn new() -> Self {
        Self {
            unlocked_levels: 0,
            pending_seed: None,
            failed_attempts: 0,
            lockout_until: None,
        }
    }

    pub fn is_unlocked(&self, level: u8) -> bool {
        self.unlocked_levels & (1 << level) != 0
    }

    pub fn reset(&mut self) {
        self.unlocked_levels = 0;
        self.pending_seed = None;
    }

    /// Handle seed request (odd subfunction). Returns seed bytes.
    pub fn request_seed(&mut self, level: u8) -> Result<Vec<u8>, Nrc> {
        if !is_seed_level(level) {
            return Err(Nrc::SubFunctionNotSupported);
        }

        if let Some(until) = self.lockout_until {
            if Instant::now() < until {
                return Err(Nrc::ExceededNumberOfAttempts);
            }
            self.lockout_until = None;
            self.failed_attempts = 0;
        }

        // Already unlocked => return zero seed
        if self.is_unlocked(level) {
            self.pending_seed = None;
            return Ok(vec![0x00, 0x00, 0x00, 0x00]);
        }

        // Generate seed using Instant as entropy source
        let now = Instant::now();
        let seed = {
            let nanos = now.elapsed().as_nanos() as u32;
            let tick = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos();
            nanos ^ tick ^ 0x1234_5678
        };

        // Ensure seed is never zero (zero = already unlocked)
        let seed = if seed == 0 { 0x0000_0001 } else { seed };

        self.pending_seed = Some((level, seed));
        Ok(seed.to_be_bytes().to_vec())
    }

    /// Handle key submission (even subfunction). Returns Ok(()) on success.
    pub fn submit_key(&mut self, level: u8, key_bytes: &[u8]) -> Result<(), Nrc> {
        if !is_key_level(level) {
            return Err(Nrc::SubFunctionNotSupported);
        }

        let seed_level = level - 1;

        let (pending_level, seed) = self.pending_seed.take().ok_or(Nrc::RequestSequenceError)?;

        if pending_level != seed_level {
            return Err(Nrc::RequestSequenceError);
        }

        if key_bytes.len() != 4 {
            return Err(Nrc::IncorrectMessageLengthOrInvalidFormat);
        }

        let submitted_key =
            u32::from_be_bytes([key_bytes[0], key_bytes[1], key_bytes[2], key_bytes[3]]);
        let expected_key = calculate_key(seed, seed_level);

        if submitted_key == expected_key {
            self.unlocked_levels |= 1 << seed_level;
            self.failed_attempts = 0;
            tracing::info!(level = seed_level, "Security access granted");
            Ok(())
        } else {
            self.failed_attempts += 1;
            tracing::warn!(
                level = seed_level,
                attempts = self.failed_attempts,
                "Invalid security key"
            );
            if self.failed_attempts >= MAX_ATTEMPTS {
                self.lockout_until =
                    Some(Instant::now() + std::time::Duration::from_millis(LOCKOUT_DURATION_MS));
                return Err(Nrc::ExceededNumberOfAttempts);
            }
            Err(Nrc::InvalidKey)
        }
    }
}

/// Fixed test key algorithm: key = seed XOR 0xDEAD_BEEF
pub fn calculate_key(seed: u32, _level: u8) -> u32 {
    seed ^ 0xDEAD_BEEF
}

fn is_seed_level(level: u8) -> bool {
    level % 2 == 1 // odd = seed request
}

fn is_key_level(level: u8) -> bool {
    level % 2 == 0 && level > 0 // even = key submission
}

impl Default for SecurityState {
    fn default() -> Self {
        Self::new()
    }
}
