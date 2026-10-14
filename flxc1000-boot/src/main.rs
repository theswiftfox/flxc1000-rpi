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

use std::ffi::CString;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;

use ace_server::config::*;
use ace_server::handler::ServerHandler;
use ace_server::nrc::BuiltinNrc;
use ace_server::security_provider::{SecurityError, SecurityProvider};
use ace_server::server::{UdsServer, MAX_FRAME, MAX_OUTBOX};
use ace_sim::clock::{Duration as AceDuration, Instant as AceInstant};
use ace_sim::io::NodeAddress;

use flxc1000_doip::server::{get_mac_address, DoipConfig, DoipServer};

const ECU_ADDRESS: u16 = 0x1000;
const VIN: &[u8; 17] = b"FLXC1000TEST00001";

mod rgb;

/// Boot variant identification: DID 0xF100 = 0xFF0000
const VARIANT_ID: [u8; 3] = [0xFF, 0x00, 0x00];

const RESET_NONE: u8 = 0;
const RESET_HARD: u8 = 1;
const RESET_SOFT: u8 = 3;

//const UPDATE_FILE: &str = "app/update.tmp";

const APP_V1_BINARY: &str = "/app/flxc1000-app-v1";
const APP_V2_BINARY: &str = "/app/flxc1000-app-v2";
const BOOT_STATE_PATH: &str = "/data/boot_state";

const APP_V1_DOWNLOAD: &str = "/app/flxc1000-app-v1.download";
const APP_V2_DOWNLOAD: &str = "/app/flxc1000-app-v2.download";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppSlot {
    V1,
    V2,
}

impl AppSlot {
    fn binary_path(self) -> &'static str {
        match self {
            AppSlot::V1 => APP_V1_BINARY,
            AppSlot::V2 => APP_V2_BINARY,
        }
    }

    fn download_path(self) -> &'static str {
        match self {
            AppSlot::V1 => APP_V1_DOWNLOAD,
            AppSlot::V2 => APP_V2_DOWNLOAD,
        }
    }

    fn other(self) -> Self {
        match self {
            AppSlot::V1 => AppSlot::V2,
            AppSlot::V2 => AppSlot::V1,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            AppSlot::V1 => "V1",
            AppSlot::V2 => "V2",
        }
    }
}

#[derive(Debug)]
struct BootState {
    active: AppSlot,
    candidate: Option<AppSlot>,
}

impl BootState {
    fn initial() -> Self {
        Self {
            active: AppSlot::V1,
            candidate: None,
        }
    }

    fn parse(contents: &str) -> Self {
        let mut state = Self::initial();

        for line in contents.lines() {
            if let Some(value) = line.strip_prefix("active=") {
                state.active = match value {
                    "V1" => AppSlot::V1,
                    "V2" => AppSlot::V2,
                    _ => state.active,
                };
            } else if let Some(value) = line.strip_prefix("candidate=") {
                state.candidate = match value {
                    "V1" => Some(AppSlot::V1),
                    "V2" => Some(AppSlot::V2),
                    _ => None,
                };
            }
        }
        state
    }

    fn to_text(&self) -> String {
        let candidate = match self.candidate {
            Some(slot) => slot.as_str(),
            None => "none",
        };

        format!(
            "active={}\ncandidate={}\nmode=application\n",
            self.active.as_str(),
            candidate
        )
    }
}

fn parse_boot_state_for_update(contents: &str) -> Result<BootState, BuiltinNrc> {
    let mut active = None;
    let mut candidate = None;
    let mut mode_found = false;

    for line in contents.lines() {
        if let Some(value) = line.strip_prefix("active=") {
            // Reject duplicate or invalid active entries.
            if active.is_some() {
                return Err(BuiltinNrc::GeneralProgrammingFailure);
            }

            active = Some(match value {
                "V1" => AppSlot::V1,
                "V2" => AppSlot::V2,
                _ => return Err(BuiltinNrc::GeneralProgrammingFailure),
            });
        } else if let Some(value) = line.strip_prefix("candidate=") {
            // Reject duplicate candidate entries.
            if candidate.is_some() {
                return Err(BuiltinNrc::GeneralProgrammingFailure);
            }

            candidate = Some(match value {
                "V1" => Some(AppSlot::V1),
                "V2" => Some(AppSlot::V2),
                "none" => None,
                _ => return Err(BuiltinNrc::GeneralProgrammingFailure),
            });
        } else if let Some(value) = line.strip_prefix("mode=") {
            if mode_found || value != "application" {
                return Err(BuiltinNrc::GeneralProgrammingFailure);
            }

            mode_found = true;
        }
    }

    let active = active.ok_or(BuiltinNrc::GeneralProgrammingFailure)?;
    let candidate = candidate.ok_or(BuiltinNrc::GeneralProgrammingFailure)?;

    if !mode_found || candidate == Some(active) {
        return Err(BuiltinNrc::GeneralProgrammingFailure);
    }

    Ok(BootState { active, candidate })
}

fn write_boot_state_to_path(state: &BootState, path: &std::path::Path) -> std::io::Result<()> {
    let temporary_path = path.with_extension("tmp");

    let mut file = File::create(&temporary_path)?;
    file.write_all(state.to_text().as_bytes())?;
    file.sync_all()?;
    drop(file);

    std::fs::rename(&temporary_path, path)?;

    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }

    Ok(())
}

fn write_boot_state_atomically(state: &BootState) -> std::io::Result<()> {
    write_boot_state_to_path(state, std::path::Path::new(BOOT_STATE_PATH))
}

fn validate_download_size(expected_size: usize, received_size: usize) -> Result<(), BuiltinNrc> {
    if expected_size == 0 || received_size != expected_size {
        return Err(BuiltinNrc::GeneralProgrammingFailure);
    }
    Ok(())
}

fn write_transfer_chunk(
    file: &mut File,
    received_size: &mut usize,
    expected_size: usize,
    data: &[u8],
) -> Result<(), BuiltinNrc> {
    // Check that this chunk fits within the expected size
    if data.len() > expected_size.saturating_sub(*received_size) {
        return Err(BuiltinNrc::GeneralProgrammingFailure);
    }

    // Write the actual bytes into the file.
    file.write_all(data)
        .map_err(|_| BuiltinNrc::GeneralProgrammingFailure)?;

    // Only update the counter after a succesfull write
    *received_size += data.len();

    Ok(())
}

fn create_staged_download(path: &std::path::Path) -> std::io::Result<File> {
    File::create(path)
}

fn clear_candidate(state: &mut BootState, path: &std::path::Path) -> std::io::Result<()> {
    let updated = BootState {
        active: state.active,
        candidate: None,
    };

    write_boot_state_to_path(&updated, path)?;

    state.candidate = None;

    Ok(())
}
#[derive(Debug, PartialEq, Eq)]
enum BlockAction {
    Write,
    Duplicate,
}

fn validate_block_sequence(
    last_accepted: Option<u8>,
    incoming: u8,
) -> Result<BlockAction, BuiltinNrc> {
    let expected = match last_accepted {
        Some(last) => last.wrapping_add(1),
        None => 1,
    };

    if incoming == expected {
        Ok(BlockAction::Write)
    } else if last_accepted == Some(incoming) {
        Ok(BlockAction::Duplicate)
    } else {
        Err(BuiltinNrc::WrongBlockSequenceCounter)
    }
}

// ---------------------------------------------------------------------------
// Shared state between handler (owned by UdsServer) and the main loop
// ---------------------------------------------------------------------------

struct SharedState {
    pending_reset: AtomicU8,
    session_type: AtomicU8,
    update_active: AtomicBool,
}

// ---------------------------------------------------------------------------
// ServerHandler — application-level UDS hooks
// ---------------------------------------------------------------------------

struct BootHandler {
    shared: Arc<SharedState>,
    transfer_file: Option<File>,
    expected_size: usize,
    received_size: usize,
    target_slot: Option<AppSlot>,
    last_block_sequence: Option<u8>,
}

impl BootHandler {
    fn reset_transfer_state(&mut self) {
        self.transfer_file = None;
        self.target_slot = None;
        self.expected_size = 0;
        self.received_size = 0;
        self.last_block_sequence = None;

        self.shared.update_active.store(false, Ordering::Relaxed);
    }
}

impl ServerHandler for BootHandler {
    type Error = BuiltinNrc;

    fn read_did(&self, did: u16, buf: &mut [u8]) -> Result<usize, BuiltinNrc> {
        match did {
            0xF100 => {
                buf[..3].copy_from_slice(&VARIANT_ID);
                Ok(3)
            }
            0xF186 => {
                buf[0] = self.shared.session_type.load(Ordering::Relaxed);
                Ok(1)
            }
            _ => Err(BuiltinNrc::RequestOutOfRange),
        }
    }

    fn write_did(&mut self, _did: u16, _data: &[u8]) -> Result<(), BuiltinNrc> {
        Err(BuiltinNrc::RequestOutOfRange)
    }

    fn ecu_reset(&mut self, reset_type: u8) -> Result<(), BuiltinNrc> {
        match reset_type {
            0x01 => {
                tracing::info!("ECUReset HardReset — will re-exec /init after response");
                self.shared
                    .pending_reset
                    .store(RESET_HARD, Ordering::SeqCst);
                Ok(())
            }
            0x03 => {
                tracing::info!("ECUReset SoftReset — will restart boot after response");
                self.shared
                    .pending_reset
                    .store(RESET_SOFT, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(BuiltinNrc::SubFunctionNotSupported),
        }
    }

    fn request_download(
        &mut self,
        _memory_address: &[u8],
        memory_size: &[u8],
        _compression_method: u8,
        _encrypting_method: u8,
        buf: &mut [u8],
    ) -> Result<usize, BuiltinNrc> {
        // Stage 1: stub positive response
        tracing::info!("RequestDownload: preparing update file");

        let expected_size = memory_size
            .iter()
            .fold(0usize, |size, &byte| (size << 8) | byte as usize);

        if expected_size == 0 {
            return Err(BuiltinNrc::RequestOutOfRange);
        }

        // Discard an abandoned previous transfer
        self.reset_transfer_state();

        let boot_state_contents = std::fs::read_to_string(BOOT_STATE_PATH).map_err(|e| {
            tracing::error!(error = %e, "Failed to read boot state");
            BuiltinNrc::GeneralProgrammingFailure
        })?;

        let boot_state = parse_boot_state_for_update(&boot_state_contents)?;

        // let target_slot = boot_state.active.other();
        // let target_path = target_slot.binary_path();
        // self.target_slot = Some(target_slot);
        let target_slot = boot_state.active.other();

        //Write to the temporary download file
        let target_path = target_slot.download_path();

        let mut boot_state = boot_state;

        clear_candidate(&mut boot_state, std::path::Path::new(BOOT_STATE_PATH)).map_err(|e| {
            tracing::error!(
                error = %e,
                "Failed to clear previous candidate"
            );
            BuiltinNrc::GeneralProgrammingFailure
        })?;

        tracing::info!(
            active = ?boot_state.active,
            target = ?target_slot,
            path = target_path,
            "RequestDownload: selected inactive application slot"
        );

        let file = create_staged_download(std::path::Path::new(target_path)).map_err(|e| {
            tracing::error!(
                error = %e,
                "Failed to create temporary download file"
            );

            BuiltinNrc::GeneralProgrammingFailure
        })?;

        self.transfer_file = Some(file);
        self.target_slot = Some(target_slot);
        self.expected_size = expected_size;
        self.received_size = 0;

        //Start the update light show
        self.shared.update_active.store(true, Ordering::Relaxed);

        // lengthFormatIdentifier: 0x20 (2 bytes for maxBlockLength)
        // maxNumberOfBlockLength: 0x0FFF (4095 bytes)
        buf[0] = 0x40;
        buf[1] = 0x00;
        buf[2] = 0x00;
        buf[3] = 0x0F;
        buf[4] = 0xFF;
        Ok(5)
    }

    fn transfer_data(
        &mut self,
        block_sequence_counter: u8,
        data: &[u8],
        _buf: &mut [u8],
    ) -> Result<usize, BuiltinNrc> {
        // TransferData requires an active download.
        if self.transfer_file.is_none() {
            return Err(BuiltinNrc::RequestSequenceError);
        }

        match validate_block_sequence(self.last_block_sequence, block_sequence_counter)? {
            BlockAction::Duplicate => {
                tracing::info!(
                    block = block_sequence_counter,
                    "TransferData: duplicate block acknowledged"
                );

                // Do not write the same block twice.
                return Ok(0);
            }

            BlockAction::Write => {}
        }

        let file = self
            .transfer_file
            .as_mut()
            .ok_or(BuiltinNrc::RequestSequenceError)?;

        write_transfer_chunk(file, &mut self.received_size, self.expected_size, data)?;

        // Only advance the sequence after the write succeeds.
        self.last_block_sequence = Some(block_sequence_counter);

        tracing::info!(
            block = block_sequence_counter,
            bytes = data.len(),
            total = self.received_size,
            "TransferData: wrote block to update file"
        );

        Ok(0)
    }

    fn request_transfer_exit(
        &mut self,
        _parameter_record: &[u8],
        _buf: &mut [u8],
    ) -> Result<usize, BuiltinNrc> {
        // 1. Take ownership of the downloaded file.
        let file = match self.transfer_file.take() {
            Some(file) => file,
            None => {
                self.reset_transfer_state();
                return Err(BuiltinNrc::RequestSequenceError);
            }
        };

        // Stop the update animation when the transfer exits,
        // including if finalization fails.
        self.shared.update_active.store(false, Ordering::Relaxed);

        // Clear the remaining transfer state before finalization
        // Keep these values locally because finalization still neds them

        let target_slot = self.target_slot.take();
        let expected_size = self.expected_size;
        let received_size = self.received_size;

        self.reset_transfer_state();

        // 2: Verify that the complete binary was received
        if let Err(error) = validate_download_size(expected_size, received_size) {
            tracing::error!(
                expected = expected_size,
                received = received_size,
                "Transfer incomplete or invalid"
            );

            return Err(error);
        }

        // 3. Sync the downloaded data to storage.
        file.sync_all().map_err(|e| {
            tracing::error!(
                error = %e,
                "Failed to sync update file"
            );

            BuiltinNrc::GeneralProgrammingFailure
        })?;

        // Close the file before finalizing it.
        drop(file);

        // 4. Retrieve the inactive application slot.
        let target_slot = target_slot.ok_or(BuiltinNrc::RequestSequenceError)?;

        let download_path = target_slot.download_path();
        let target_path = target_slot.binary_path();

        // 5. Make the staged application executable.
        let permissions = std::fs::Permissions::from_mode(0o755);

        std::fs::set_permissions(download_path, permissions).map_err(|e| {
            tracing::error!(
                error = %e,
                "Failed to set downloaded application permissions"
            );

            BuiltinNrc::GeneralProgrammingFailure
        })?;

        // 6. Sync the file again after changing permissions.
        File::open(download_path)
            .and_then(|file| file.sync_all())
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    "Failed to sync staged executable"
                );

                BuiltinNrc::GeneralProgrammingFailure
            })?;

        // 7. Read and verify the current boot state before publishing.
        let boot_state_contents = std::fs::read_to_string(BOOT_STATE_PATH).map_err(|e| {
            tracing::error!(
                error = %e,
                "Failed to read boot state"
            );

            BuiltinNrc::GeneralProgrammingFailure
        })?;

        let mut boot_state = parse_boot_state_for_update(&boot_state_contents)?;

        // Never replace the currently active application.
        if boot_state.active == target_slot {
            tracing::error!("Target slot became active during download");

            return Err(BuiltinNrc::GeneralProgrammingFailure);
        }

        // 8. Finalize the binary in the inactive slot.
        std::fs::rename(download_path, target_path).map_err(|e| {
            tracing::error!(
                error = %e,
                "Failed to finalize application binary"
            );

            BuiltinNrc::GeneralProgrammingFailure
        })?;

        // 9. Persist the filesystem rename.
        File::open("/app")
            .and_then(|dir| dir.sync_all())
            .map_err(|e| {
                tracing::error!(
                    error = %e,
                    "Failed to sync application directory"
                );

                BuiltinNrc::GeneralProgrammingFailure
            })?;

        // 10. Mark the completed application as candidate.
        boot_state.candidate = Some(target_slot);

        write_boot_state_atomically(&boot_state).map_err(|e| {
            tracing::error!(
                error = %e,
                "Failed to save candidate boot state"
            );

            BuiltinNrc::GeneralProgrammingFailure
        })?;

        tracing::info!(
            target = ?target_slot,
            expected = expected_size,
            received = received_size,
            "TransferExit: application finalized successfully"
        );

        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// SecurityProvider — seed/key with XOR 0xDEADBEEF
// ---------------------------------------------------------------------------

struct BootSecurity;

impl SecurityProvider for BootSecurity {
    fn generate_seed(&mut self, _level: u8, buf: &mut [u8]) -> Result<usize, SecurityError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u32;
        let seed = now.wrapping_mul(2654435761); // Knuth multiplicative hash
        buf[..4].copy_from_slice(&seed.to_be_bytes());
        Ok(4)
    }

    fn validate_key(&self, _level: u8, seed: &[u8], key: &[u8]) -> Result<(), SecurityError> {
        if seed.len() < 4 || key.len() < 4 {
            return Err(SecurityError::InvalidKey);
        }
        let seed_u32 = u32::from_be_bytes([seed[0], seed[1], seed[2], seed[3]]);
        let expected = seed_u32 ^ 0xDEAD_BEEF;
        let actual = u32::from_be_bytes([key[0], key[1], key[2], key[3]]);
        if actual == expected {
            Ok(())
        } else {
            Err(SecurityError::InvalidKey)
        }
    }
}

// ---------------------------------------------------------------------------
// Build the ace-server configuration for the Boot variant
// ---------------------------------------------------------------------------

fn boot_server_config() -> ServerConfig {
    ServerConfig::new(ECU_ADDRESS, 0xFFFF)
        // Sessions
        .with_session(SessionConfig::default_session())
        .with_session(SessionConfig::programming_session())
        .with_session(SessionConfig::extended_session())
        // Services: (SID, allowed sessions)
        .with_service(ServiceConfig::new(0x10, &[0x01, 0x02, 0x03])) // DiagnosticSessionControl
        .with_service(ServiceConfig::new(0x11, &[0x01, 0x02, 0x03])) // ECUReset
        .with_service(ServiceConfig::new(0x22, &[0x01, 0x02, 0x03])) // ReadDataByIdentifier
        .with_service(ServiceConfig::new(0x27, &[0x02, 0x03])) // SecurityAccess
        .with_service(ServiceConfig::secured(0x34, &[0x02], 0x03)) // RequestDownload (programming, sec 03)
        .with_service(ServiceConfig::secured(0x36, &[0x02], 0x03)) // TransferData
        .with_service(ServiceConfig::secured(0x37, &[0x02], 0x03)) // RequestTransferExit
        .with_service(ServiceConfig::new(0x3E, &[0x01, 0x02, 0x03])) // TesterPresent
        // DIDs
        .with_did(DidConfig::read_only(0xF100, &[0x01, 0x02, 0x03])) // variant ID
        .with_did(DidConfig::read_only(0xF186, &[0x01, 0x02, 0x03])) // session
        // Security levels
        .with_security_level(SecurityLevelConfig {
            level: 0x03,
            max_attempts: 3,
            lockout_duration: AceDuration::from_secs(10),
            seed_length: 4,
            key_length: 4,
        })
}

// ---------------------------------------------------------------------------
// Reset helpers
// ---------------------------------------------------------------------------

fn do_hard_reset() -> ! {
    tracing::info!("Performing real system reboot");

    match nix::sys::reboot::reboot(nix::sys::reboot::RebootMode::RB_AUTOBOOT) {
        Ok(infallible) => match infallible {},
        Err(e) => {
            tracing::error!("System reboot failed: {}", e);
            process::exit(1);
        }
    }
}

fn do_soft_reset() -> ! {
    let exe = std::env::current_exe().unwrap_or_else(|_| "/boot/flxc1000-boot".into());
    let c_path = CString::new(exe.to_string_lossy().as_ref()).expect("invalid path");
    let argv = [c_path.clone()];
    let env: Vec<CString> = vec![
        CString::new("PATH=/bin:/sbin").unwrap(),
        CString::new("HOME=/").unwrap(),
    ];
    match nix::unistd::execve(&c_path, &argv, &env) {
        Ok(infallible) => match infallible {},
        Err(e) => {
            tracing::error!("execve self failed: {}", e);
            process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// ace_sim::clock helpers — convert std::time to ace Instant
// ---------------------------------------------------------------------------

fn ace_now(boot: &std::time::Instant) -> AceInstant {
    AceInstant::from_micros(boot.elapsed().as_micros() as u64)
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    tracing::info!(
        "flxc1000-boot starting (Boot variant, address 0x{:04X})",
        ECU_ADDRESS
    );

    // if let Err(e) = rgb::white_test() {
    //     tracing::warn!("RGB boot test failed: {}",e);
    // }

    let mac = get_mac_address();
    tracing::info!(
        "EID (MAC): {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );

    let doip_config = DoipConfig::new(ECU_ADDRESS, VIN).with_eid(mac);
    let doip = DoipServer::new(doip_config);
    let mut uds_rx = doip.run().await.expect("Failed to start DoIP server");

    let shared = Arc::new(SharedState {
        pending_reset: AtomicU8::new(RESET_NONE),
        session_type: AtomicU8::new(0x01),
        update_active: AtomicBool::new(false),
    });

    rgb::spawn_update_task(Arc::clone(&shared));

    let handler = BootHandler {
        shared: Arc::clone(&shared),
        transfer_file: None,
        expected_size: 0,
        received_size: 0,
        target_slot: None,
        last_block_sequence: None,
    };

    let mut server = UdsServer::new(
        boot_server_config(),
        handler,
        BootSecurity,
        NodeAddress(ECU_ADDRESS as u32),
    );

    let boot_time = std::time::Instant::now();
    tracing::info!("Boot ECU ready, waiting for diagnostic requests");

    while let Some(req) = uds_rx.recv().await {
        tracing::debug!(
            src = format_args!("0x{:04X}", req.source_address),
            tgt = format_args!("0x{:04X}", req.target_address),
            data = format_args!("{:02X?}", &req.data),
            "UDS request"
        );

        let now = ace_now(&boot_time);

        // Keep session_type in shared state up to date for DID 0xF186
        shared
            .session_type
            .store(server.session_type(), Ordering::Relaxed);

        let src = NodeAddress(req.source_address as u32);
        let _ = server.handle(&src, &req.data, now);
        let _ = server.tick(now);

        // Update session_type after handle (may have changed)
        shared
            .session_type
            .store(server.session_type(), Ordering::Relaxed);

        // Drain outbox → collect response for this tester
        let mut outbox: heapless::Vec<(NodeAddress, heapless::Vec<u8, MAX_FRAME>), MAX_OUTBOX> =
            heapless::Vec::new();
        server.drain_outbox(&mut outbox);

        let response: Vec<u8> = outbox
            .into_iter()
            .find(|(dst, _)| dst == &src)
            .map(|(_, data)| data.to_vec())
            .unwrap_or_default();

        tracing::debug!(data = format_args!("{:02X?}", &response), "UDS response");

        if let Err(e) = req.response_tx.send(response).await {
            tracing::error!("Failed to send UDS response: {}", e);
        }

        // Check for pending reset AFTER response is sent
        let reset = shared.pending_reset.swap(RESET_NONE, Ordering::SeqCst);
        match reset {
            RESET_HARD => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                do_hard_reset();
            }
            RESET_SOFT => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                do_soft_reset();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_complete_download() {
        let expected_size = 1000;
        let received_size = 1000;

        let result = validate_download_size(expected_size, received_size);

        assert!(result.is_ok());
    }

    #[test]
    fn test_interrupted_download() {
        let expected_size = 1000;
        let received_size = 400;

        let result = validate_download_size(expected_size, received_size);

        assert!(result.is_err());
    }

    #[test]
    fn test_oversized_download() {
        let expected_size = 1000;
        let received_size = 1200;

        let result = validate_download_size(expected_size, received_size);

        assert!(result.is_err());
    }

    #[test]
    fn test_empty_download() {
        let expected_size = 0;
        let received_size = 0;

        let result = validate_download_size(expected_size, received_size);

        assert!(result.is_err());
    }

    #[test]
    fn test_real_file_transfer() {
        let path = std::env::temp_dir().join(format!("flxc-complete-{}.bin", std::process::id()));

        let mut file = File::create(&path).unwrap();

        let expected_size = 1000;
        let mut received_size = 0;

        //Simulate two UDS TransferData chunks
        let chunk1 = vec![0xAA; 500];
        let chunk2 = vec![0xBB; 500];

        write_transfer_chunk(&mut file, &mut received_size, expected_size, &chunk1).unwrap();

        write_transfer_chunk(&mut file, &mut received_size, expected_size, &chunk2).unwrap();

        file.sync_all().unwrap();
        drop(file);

        //Check the actual data stored on the disk
        let contents = std::fs::read(&path).unwrap();

        assert_eq!(contents.len(), 1000);
        assert_eq!(&contents[..500], chunk1.as_slice());
        assert_eq!(&contents[500..], chunk2.as_slice());
        assert_eq!(received_size, 1000);

        assert!(validate_download_size(expected_size, received_size).is_ok());

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_real_interrupted_transfer() {
        let path =
            std::env::temp_dir().join(format!("flxc-interrupted-{}.bin", std::process::id()));

        let mut file = File::create(&path).unwrap();

        let expected_size = 1000;
        let mut received_size = 0;

        //Only 40% of the application arrives
        let chunk = vec![0xAA; 400];

        write_transfer_chunk(&mut file, &mut received_size, expected_size, &chunk).unwrap();

        file.sync_all().unwrap();
        drop(file);

        //Verify all 400 bytes were stored
        let contents = std::fs::read(&path).unwrap();

        assert_eq!(contents.len(), 400);
        assert_eq!(contents, chunk);
        assert_eq!(received_size, 400);

        //Finalization must reject the incomplete download
        assert!(validate_download_size(expected_size, received_size).is_err());

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn test_download_paths_are_separated() {
        assert_ne!(AppSlot::V1.download_path(), AppSlot::V1.binary_path());

        assert_ne!(AppSlot::V2.download_path(), AppSlot::V2.binary_path());
    }

    #[test]
    fn test_v1_v2_download_paths() {
        assert_eq!(AppSlot::V1.download_path(), "/app/flxc1000-app-v1.download");

        assert_eq!(AppSlot::V2.download_path(), "/app/flxc1000-app-v2.download");
    }

    #[test]
    fn test_interrupted_download_restart() {
        use std::fs;

        let test_dir = std::env::temp_dir().join(format!("flxc-restart-{}", std::process::id()));

        fs::create_dir_all(&test_dir).unwrap();

        let final_path = test_dir.join("app-v2");
        let download_path = test_dir.join("app-v2.download");

        let original = vec![0x11; 1000];
        fs::write(&final_path, &original).unwrap();

        let expected_size = 1000;
        let mut received_size = 0;

        // First attempt: only 400 bytes arrive.
        let mut file = create_staged_download(&download_path).unwrap();

        write_transfer_chunk(
            &mut file,
            &mut received_size,
            expected_size,
            &vec![0xAA; 400],
        )
        .unwrap();

        drop(file);

        assert_eq!(fs::metadata(&download_path).unwrap().len(), 400);
        assert_eq!(fs::read(&final_path).unwrap(), original);

        // Second attempt: start again.
        let mut file = create_staged_download(&download_path).unwrap();
        received_size = 0;

        let new_binary = vec![0xBB; 1000];

        write_transfer_chunk(&mut file, &mut received_size, expected_size, &new_binary).unwrap();

        file.sync_all().unwrap();
        drop(file);

        assert_eq!(fs::read(&download_path).unwrap(), new_binary);
        assert!(validate_download_size(expected_size, received_size).is_ok());

        // The finalized executable is still unchanged.
        assert_eq!(fs::read(&final_path).unwrap(), original);

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn test_interrupted_download_preserves_active_slot() {
        use std::fs;

        let test_dir = std::env::temp_dir().join(format!("flxc-state-test-{}", std::process::id()));

        fs::create_dir_all(&test_dir).unwrap();

        let state_path = test_dir.join("boot_state");

        //Start with V1 active and V2 selected as candidate
        let mut state = BootState {
            active: AppSlot::V1,
            candidate: Some(AppSlot::V2),
        };

        write_boot_state_to_path(&state, &state_path).unwrap();

        //Simulate RequestDownload preparing for another V2 update
        clear_candidate(&mut state, &state_path).unwrap();

        //Simulate an expected interruption
        drop(state);

        //Read the persisted state as fresh process would.
        let contents = fs::read_to_string(&state_path).unwrap();
        let restored = BootState::parse(&contents);

        assert_eq!(restored.active, AppSlot::V1);
        assert_eq!(restored.candidate, None);

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn test_valid_boot_state_for_update() {
        let text = "active=V2\ncandidate=none\nmode=application\n";

        let state = parse_boot_state_for_update(text).unwrap();

        assert_eq!(state.active, AppSlot::V2);
        assert_eq!(state.candidate, None);
    }

    #[test]
    fn test_invalid_active_slot_rejected() {
        let text = "active=INVALID\ncandidate=none\nmode=application\n";

        assert!(parse_boot_state_for_update(text).is_err());
    }

    #[test]
    fn test_missing_active_slot_rejected() {
        let text = "active=V1\ncandidate=V1\nmode=application\n";

        assert!(parse_boot_state_for_update(text).is_err());
    }

    #[test]
    fn test_duplicate_active_slot_rejected() {
        let text = "active=V1\nactive=V2\ncandidate=none\nmode=application\n";

        assert!(parse_boot_state_for_update(text).is_err());
    }

    #[test]
    fn test_first_block_must_be_one() {
        assert_eq!(
            validate_block_sequence(None, 1).unwrap(),
            BlockAction::Write
        );

        assert!(validate_block_sequence(None, 2).is_err());
    }

    #[test]
    fn test_blocks_in_order() {
        assert_eq!(
            validate_block_sequence(Some(1), 2).unwrap(),
            BlockAction::Write
        );
    }

    #[test]
    fn test_duplicate_block() {
        assert_eq!(
            validate_block_sequence(Some(2), 2).unwrap(),
            BlockAction::Duplicate
        );
    }

    #[test]
    fn test_out_of_order_block() {
        assert!(validate_block_sequence(Some(2), 4).is_err());
    }

    #[test]
    fn test_block_counter_wraparound() {
        assert_eq!(
            validate_block_sequence(Some(255), 0).unwrap(),
            BlockAction::Write
        );
    }
}
