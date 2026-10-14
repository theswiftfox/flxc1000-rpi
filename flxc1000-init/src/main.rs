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

// This binary is Linux-only (PID 1 init for RPi4 initramfs).
// It will not compile on macOS — cross-compile with:
//   cargo zigbuild --target aarch64-unknown-linux-musl --release -p flxc1000-init
const APP_V1_BINARY: &str = "/app/flxc1000-app-v1";
const APP_V2_BINARY: &str = "/app/flxc1000-app-v2";

use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppSlot {
    V1,
    V2,
}

#[derive(Debug)]
struct BootState {
    active: AppSlot,
    candidate: Option<AppSlot>,
    bootloader_requested: bool,
}

impl AppSlot {
    fn binary_path(self) -> &'static str {
        match self {
            AppSlot::V1 => APP_V1_BINARY,
            AppSlot::V2 => APP_V2_BINARY,
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "V1" => Some(AppSlot::V1),
            "V2" => Some(AppSlot::V2),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            AppSlot::V1 => "V1",
            AppSlot::V2 => "V2",
        }
    }
}

impl BootState {
    fn initial() -> Self {
        Self {
            active: AppSlot::V1,
            candidate: None,
            bootloader_requested: false,
        }
    }

    fn parse(contents: &str) -> Self {
        let mut state = Self::initial();

        for line in contents.lines() {
            if let Some(value) = line.strip_prefix("active=") {
                if let Some(slot) = AppSlot::from_str(value) {
                    state.active = slot;
                }
            } else if let Some(value) = line.strip_prefix("candidate=") {
                state.candidate = match value {
                    "none" => None,
                    other => AppSlot::from_str(other),
                };
            } else if let Some(value) = line.strip_prefix("mode=") {
                state.bootloader_requested = value == "bootloader";
            }
        }

        state
    }

    fn to_text(&self) -> String {
        let candidate = match self.candidate {
            Some(slot) => slot.as_str(),
            None => "none",
        };

        let mode = if self.bootloader_requested {
            "bootloader"
        } else {
            "application"
        };

        format!(
            "active={}\ncandidate={}\nmode={}\n",
            self.active.as_str(),
            candidate,
            mode
        )
    }
}

fn apply_candidate_result(state: &mut BootState, succesful: bool) {
    if let Some(candidate) = state.candidate {
        if succesful {
            state.active = candidate;
        }

        state.candidate = None;
        state.bootloader_requested = false;
    }
}

fn test_candidate(path: &str) -> bool {
    eprintln!("[init] Starting candidate application: {}", path);

    let mut child = match Command::new(path).spawn() {
        Ok(child) => child,
        Err(e) => {
            eprintln!("[init] Failed to start candidate: {}", e);
            return false;
        }
    };

    let deadline = Instant::now() + Duration::from_secs(2);

    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => {
                eprintln!("[init] Candidate exited during startup test: {}", status);
                return false;
            }

            Ok(None) => {
                // Candidate is still running.
            }

            Err(e) => {
                eprintln!("[init] Failed to check candidate: {}", e);
                return false;
            }
        }

        std::thread::sleep(Duration::from_millis(100));
    }

    eprintln!("[init] Candidate survived startup test");

    // Stop the temporary validation process.
    if let Err(e) = child.kill() {
        eprintln!("[init] Failed to stop candidate test process: {}", e);
        return false;
    }

    // Reap the stopped child process.
    if let Err(e) = child.wait() {
        eprintln!("[init] Failed to reap candidate test process: {}", e);
        return false;
    }

    true
}

#[cfg(target_os = "linux")]
fn main() {
    use nix::mount::{mount, MsFlags};
    use nix::unistd::execve;
    use std::ffi::CString;
    use std::fs;
    use std::path::Path;

    const DATA_PARTITION: &str = "/dev/mmcblk0p3";
    const DATA_MOUNT: &str = "/data";
    const APP_PARTITION: &str = "/dev/mmcblk0p2";
    const APP_MOUNT: &str = "/app";
    const BOOT_STATE_PATH: &str = "/data/boot_state";
    const BOOT_BINARY: &str = "/boot/flxc1000-boot";
    const MMC_TIMEOUT: Duration = Duration::from_secs(10);

    fn write_boot_state(state: &BootState) -> Result<(), std::io::Error> {
        fs::write(BOOT_STATE_PATH, state.to_text())
    }

    fn mount_pseudo_fs() {
        let _ = fs::create_dir_all("/proc");
        let _ = mount(
            Some("proc"),
            "/proc",
            Some("proc"),
            MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            None::<&str>,
        );

        let _ = fs::create_dir_all("/sys");
        let _ = mount(
            Some("sysfs"),
            "/sys",
            Some("sysfs"),
            MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
            None::<&str>,
        );

        let _ = fs::create_dir_all("/dev");
        let _ = mount(
            Some("devtmpfs"),
            "/dev",
            Some("devtmpfs"),
            MsFlags::empty(),
            None::<&str>,
        );

        // Required for interactive SSH terminals
        let _ = fs::create_dir_all("/dev/pts");
        let _ = mount(
            Some("devpts"),
            "/dev/pts",
            Some("devpts"),
            MsFlags::empty(),
            None::<&str>,
        );
    }

    fn mount_partition(device: &str, target: &str, fstype: &str) -> Result<(), String> {
        let _ = fs::create_dir_all(target);
        mount(
            Some(device),
            target,
            Some(fstype),
            MsFlags::empty(),
            None::<&str>,
        )
        .map_err(|e| format!("mount {} -> {}: {}", device, target, e))
    }

    fn wait_for_mmc_partitions() -> bool {
        let deadline = Instant::now() + MMC_TIMEOUT;
        while Instant::now() < deadline {
            if Path::new(APP_PARTITION).exists() && Path::new(DATA_PARTITION).exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    fn exec_binary(path: &str) -> ! {
        let c_path = CString::new(path).expect("invalid path");
        let argv = [c_path.clone()];
        let env: Vec<CString> = vec![
            CString::new("PATH=/bin:/sbin").unwrap(),
            CString::new("HOME=/").unwrap(),
        ];
        match execve(&c_path, &argv, &env) {
            Ok(infallible) => match infallible {},
            Err(e) => {
                eprintln!("[init] execve({}) failed: {}", path, e);
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                }
            }
        }
    }

    eprintln!("[init] flxc1000-init starting as PID 1");

    mount_pseudo_fs();

    // MMC probing is asynchronous, and this initramfs starts before the
    // card partitions necessarily appear in /dev.
    if !wait_for_mmc_partitions() {
        eprintln!("[init] MMC partitions did not appear — falling back to boot");
        exec_binary(BOOT_BINARY);
    }

    if let Err(e) = mount_partition(DATA_PARTITION, DATA_MOUNT, "ext4") {
        eprintln!("[init] Failed to mount /data: {} — falling back to boot", e);
        exec_binary(BOOT_BINARY);
    }

    if let Err(e) = mount_partition(APP_PARTITION, APP_MOUNT, "ext4") {
        eprintln!("[init] Failed to mount /app: {} — falling back to boot", e);
        exec_binary(BOOT_BINARY);
    }

    let boot_state_contents = fs::read_to_string(BOOT_STATE_PATH).unwrap_or_default();

    let boot_state = BootState::parse(&boot_state_contents);

    eprintln!("[init] boot_state = {:?}", boot_state);

    // Start SSH Server
    if let Err(e) = Command::new("/usr/sbin/sshd").spawn() {
        eprintln!("Failed to start start openssh server {:?}", e);
    } else {
        eprintln!("Succeded to start start openssh server");
    }

    if boot_state.bootloader_requested {
        eprintln!("[init] Bootloader requested");
        exec_binary(BOOT_BINARY);
    }

    let mut boot_state = boot_state;

    if let Some(candidate) = boot_state.candidate {
        let candidate_binary = candidate.binary_path();

        eprintln!(
            "[init] Candidate {:?} found: {}",
            candidate, candidate_binary
        );

        if Path::new(candidate_binary).exists() && test_candidate(candidate_binary) {
            eprintln!("[init] Candidate {:?} passed startup test", candidate);

            // boot_state.active = candidate;
            // boot_state.candidate = None;
            // boot_state.bootloader_requested = false;
            apply_candidate_result(&mut boot_state, true);

            if let Err(e) = write_boot_state(&boot_state) {
                eprintln!("[init] Failed to save successful candidate: {}", e);
                exec_binary(BOOT_BINARY);
            }

            eprintln!("[init] Candidate {:?} is now active", candidate);
        } else {
            eprintln!(
                "[init] Candidate {:?} failed — falling back to {:?}",
                candidate, boot_state.active
            );

            // boot_state.candidate = None;
            // boot_state.bootloader_requested = false;
            apply_candidate_result(&mut boot_state, false);

            if let Err(e) = write_boot_state(&boot_state) {
                eprintln!("[init] Failed to save rollback state: {}", e);
                exec_binary(BOOT_BINARY);
            }
        }
    }

    let active_binary = boot_state.active.binary_path();

    if Path::new(active_binary).exists() {
        eprintln!(
            "[init] Executing active application {:?}: {}",
            boot_state.active, active_binary
        );

        exec_binary(active_binary);
    } else {
        eprintln!(
            "[init] Active application {:?} not found — falling back to bootloader",
            boot_state.active
        );

        exec_binary(BOOT_BINARY);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("flxc1000-init is Linux-only. Cross-compile with:");
    eprintln!("  cargo zigbuild --target aarch64-unknown-linux-musl --release -p flxc1000-init");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_boot_state_parsing() {
        let state = BootState::parse("active=V2\ncandidate=V1\nmode=application");

        // Simulate failed candidate validation
        assert_eq!(state.active, AppSlot::V2);
        assert_eq!(state.candidate, Some(AppSlot::V1));
        assert!(!state.bootloader_requested);
    }

    #[test]
    fn test_succesful_update_state() {
        let mut state = BootState::parse("active=V2\ncandidate=V1\nmode=application");

        apply_candidate_result(&mut state, true);

        assert_eq!(state.active, AppSlot::V1);
        assert_eq!(state.candidate, None);
        assert!(!state.bootloader_requested);
    }

    #[test]
    fn test_rollback_state() {
        let mut state = BootState::parse("active=V2\ncandidate=V1\nmode=application\n");

        apply_candidate_result(&mut state, false);

        assert_eq!(state.active, AppSlot::V2);
        assert_eq!(state.candidate, None);
        assert!(!state.bootloader_requested);
    }

    #[test]
    fn test_boot_state_serialization() {
        let state = BootState::parse("active=V2\ncandidate=none\nmode=application\n");

        assert_eq!(
            state.to_text(),
            "active=V2\ncandidate=none\nmode=application\n"
        );
    }

    #[test]
    fn test_v1_to_v2_success() {
        let mut state = BootState::parse("active=V1\ncandidate=V2\nmode=application\n");

        apply_candidate_result(&mut state, true);

        assert_eq!(state.active, AppSlot::V2);
        assert_eq!(state.candidate, None);
        assert!(!state.bootloader_requested);
    }

    #[test]
    fn test_v1_to_v2_rollback() {
        let mut state = BootState::parse("active=V1\ncandidate=V2\nmode=application\n");

        apply_candidate_result(&mut state, false);

        assert_eq!(state.active, AppSlot::V1);
        assert_eq!(state.candidate, None);
        assert!(!state.bootloader_requested);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_candidate_accepts_running_application() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir();
        let path = dir.join(format!("flxc-good-{}", std::process::id()));

        std::fs::write(&path, "#!/bin/sh\nexec sleep 10\n").unwrap();

        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();

        let result = test_candidate(path.to_str().unwrap());

        std::fs::remove_file(&path).unwrap();

        assert!(result, "Working candidate should be accepted");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_candidate_rejects_crashing_application() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir();
        let path = dir.join(format!("flxc-bad-{}", std::process::id()));

        std::fs::write(&path, "#!/bin/sh\nexit 1\n").unwrap();

        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&path, permissions).unwrap();

        let result = test_candidate(path.to_str().unwrap());

        std::fs::remove_file(&path).unwrap();

        assert!(!result, "Crashing candidate should be rejected");
    }

    #[cfg(target_os = "linux")]
    #[test]

    fn test_candidate_rejects_missing_applications() {
        let result = test_candidate("/tmp/flxc1000-nonexistent-test-application");

        assert!(!result, "Missing candidate should be rejected");
    }
}
