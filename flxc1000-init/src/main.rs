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

fn consume_bootloader_request(contents: &str) -> std::io::Result<Option<String>> {
    use std::io::{Error, ErrorKind};

    let invalid = |message| Error::new(ErrorKind::InvalidData, message);

    let mut active = None;
    let mut candidate = None;
    let mut mode = None;

    let mut offset = 0;
    for raw_line in contents.split_inclusive('\n') {
        let line = raw_line.lines().next().unwrap_or_default();
        if let Some(value) = line.strip_prefix("active=") {
            if active.is_some() || AppSlot::from_str(value).is_none() {
                return Err(invalid("invalid or duplicate active slot"));
            }
            active = Some(value);
        } else if let Some(value) = line.strip_prefix("candidate=") {
            if candidate.is_some() || (value != "none" && AppSlot::from_str(value).is_none()) {
                return Err(invalid("invalid or duplicate candidate slot"));
            }
            candidate = Some(value);
        } else if let Some(value) = line.strip_prefix("mode=") {
            if mode.is_some() || !matches!(value, "application" | "bootloader") {
                return Err(invalid("invalid or duplicate boot mode"));
            }
            mode = Some((value, offset + "mode=".len()));
        }
        offset += raw_line.len();
    }

    let active = active.ok_or_else(|| invalid("missing active slot"))?;
    let candidate = candidate.ok_or_else(|| invalid("missing candidate slot"))?;
    let (mode, mode_offset) = mode.ok_or_else(|| invalid("missing boot mode"))?;

    if candidate == active {
        return Err(invalid("candidate matches active slot"));
    }

    if mode == "application" {
        return Ok(None);
    }

    let mut updated = contents.to_owned();
    updated.replace_range(mode_offset..mode_offset + mode.len(), "application");
    Ok(Some(updated))
}

#[cfg(target_os = "linux")]
fn persist_consumed_request(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::fs::{self, File};
    use std::io::Write;
    let temporary_path = path.with_extension("handoff.tmp");

    let mut file = File::create(&temporary_path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    drop(file);

    fs::rename(&temporary_path, path)?;

    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }

    Ok(())
}

#[cfg(target_os = "linux")]
#[derive(Debug)]
enum HandoffOutcome {
    StartBootloader,
    RestartActive(AppSlot),
    ContinueNormalStartup(BootState),
    Unsafe,
}

#[cfg(target_os = "linux")]
fn handle_bootloader_handoff(
    path: &std::path::Path,
    mut persist: impl FnMut(&std::path::Path, &str) -> std::io::Result<()>,
) -> HandoffOutcome {
    use std::fs::read_to_string;

    let original = match read_to_string(path) {
        Ok(contents) => contents,
        Err(e) => {
            eprintln!("[init] Failed to read boot state: {e}");
            return HandoffOutcome::Unsafe;
        }
    };
    let expected = match consume_bootloader_request(&original) {
        Ok(Some(updated)) => updated,
        Ok(None) => return HandoffOutcome::ContinueNormalStartup(BootState::parse(&original)),
        Err(e) => {
            eprintln!("[init] Invalid boot state: {e}");
            return HandoffOutcome::Unsafe;
        }
    };

    match persist(path, &expected) {
        Ok(()) => return HandoffOutcome::StartBootloader,
        Err(e) => eprintln!("[init] Failed to persist bootloader handoff: {e}"),
    }

    match read_to_string(path) {
        Ok(actual) if actual == expected => {
            eprintln!("[init] Updated boot state is visible, but power-loss durability is unconfirmed");
            return HandoffOutcome::StartBootloader;
        }
        Ok(actual) if actual == original => {}
        Ok(_) => {
            eprintln!("[init] Unexpected boot state after handoff failure");
            return HandoffOutcome::Unsafe;
        }
        Err(e) => {
            eprintln!("[init] Could not read back boot state after handoff failure: {e}");
            return HandoffOutcome::Unsafe;
        }
    }

    // The original validated request is still present. Try to cancel it once.
    // Restart only after the write succeeds and exact read-back confirms the request is cleared.
    if let Err(e) = persist(path, &expected) {
        eprintln!("[init] Failed to persist update cancellation: {e}");
        return HandoffOutcome::Unsafe;
    }

    match read_to_string(path) {
        Ok(actual) if actual == expected => {
            HandoffOutcome::RestartActive(BootState::parse(&original).active)
        }
        Ok(_) => {
            eprintln!("[init] Boot state did not match the expected cancellation");
            HandoffOutcome::Unsafe
        }
        Err(e) => {
            eprintln!("[init] Could not verify update cancellation: {e}");
            HandoffOutcome::Unsafe
        }
    }
}

fn check_existing_mount(
    mountinfo: &str,
    device: &str,
    target: &str,
    fstype: &str,
) -> Result<bool, String> {
    let mut found = false;

    for line in mountinfo.lines() {
        let Some((before, after)) = line.split_once(" - ") else {
            return Err("Malformed mountinfo entry".to_string());
        };

        let fields: Vec<&str> = before.split_whitespace().collect();
        let details: Vec<&str> = after.split_whitespace().collect();

        if fields.len() < 6 || details.len() < 3 {
            return Err("Incomplete mountinfo entry".to_string());
        }

        let mounted_target = fields[4];
        let mounted_fstype = details[0];
        let mounted_device = details[1];

        if mounted_target == target {
            if found {
                return Err(format!("Multiple mounts detected at {}", target));
            }

            found = true;

            if mounted_device != device || mounted_fstype != fstype {
                return Err(format!(
                    "Unexpected mount at {}: {} ({})",
                    target, mounted_device, mounted_fstype
                ));
            }
        }
    }

    Ok(found)
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
fn hold_on_unsafe_boot_state() -> ! {
    eprintln!("[init] FATAL: boot state cannot be verified; refusing to continue");

    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
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
        let mountinfo = fs::read_to_string("/proc/self/mountinfo")
            .map_err(|e| format!("Failed to read mountinfo: {}", e))?;

        if check_existing_mount(&mountinfo, device, target, fstype)? {
            eprintln!("[init] {} already mounted correctly at {}", device, target);
            return Ok(());
        }

        fs::create_dir_all(target)
            .map_err(|e| format!("Failed to create mount point {}: {}", target, e))?;
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

    // Start SSH Server
    if let Err(e) = Command::new("/usr/sbin/sshd").spawn() {
        eprintln!("Failed to start start openssh server {:?}", e);
    } else {
        eprintln!("Succeded to start start openssh server");
    }

    let mut boot_state = match handle_bootloader_handoff(
        Path::new(BOOT_STATE_PATH),
        persist_consumed_request,
    ) {
        HandoffOutcome::StartBootloader => {
            eprintln!("[init] Bootloader request consumed");
            exec_binary(BOOT_BINARY);
        }
        HandoffOutcome::RestartActive(slot) => {
            let path = slot.binary_path();
            if !Path::new(path).is_file() {
                eprintln!("[init] Active application binary missing: {path}");
                hold_on_unsafe_boot_state();
            }
            eprintln!("[init] Update handoff cancelled; restarting {slot:?}");
            exec_binary(path);
        }
        HandoffOutcome::ContinueNormalStartup(state) => state,
        HandoffOutcome::Unsafe => hold_on_unsafe_boot_state(),
    };

    eprintln!("[init] boot_state = {:?}", boot_state);

    if let Some(candidate) = boot_state.candidate {
        let candidate_binary = candidate.binary_path();

        eprintln!(
            "[init] Candidate {:?} found: {}",
            candidate, candidate_binary
        );

        if Path::new(candidate_binary).exists() && test_candidate(candidate_binary) {
            eprintln!("[init] Candidate {:?} passed startup test", candidate);

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

    #[test]
    fn test_existing_mount_matches() {
        let mountinfo = "36 25 179:3 / /data rw,relatime - ext4 /dev/mmcblk0p3 rw\n";

        let result = check_existing_mount(mountinfo, "/dev/mmcblk0p3", "/data", "ext4");

        assert_eq!(result.unwrap(), true);
    }

    #[test]
    fn test_existing_mount_not_found() {
        let mountinfo = "36 25 179:3 / /other rw,relatime - ext4 /dev/mmcblk0p3 rw\n";

        let result = check_existing_mount(mountinfo, "/dev/mmcblk0p3", "/data", "ext4");

        assert_eq!(result.unwrap(), false);
    }

    #[test]
    fn test_existing_mount_wrong_device() {
        let mountinfo = "36 25 179:2 / /data rw,relatime - ext4 /dev/mmcblk0p2 rw\n";

        let result = check_existing_mount(mountinfo, "/dev/mmcblk0p3", "/data", "ext4");

        assert!(result.is_err());
    }

    #[test]
    fn test_existing_mount_wrong_filesystem() {
        let mountinfo = "36 25 179:3 / /data rw,relatime - vfat /dev/mmcblk0p3 rw\n";

        let result = check_existing_mount(mountinfo, "/dev/mmcblk0p3", "/data", "ext4");

        assert!(result.is_err());
    }

    #[test]
    fn test_existing_mount_duplicate_target() {
        let mountinfo = concat!(
            "36 25 179:3 / /data rw,relatime - ext4 /dev/mmcblk0p3 rw\n",
            "42 36 179:3 / /data rw,relatime - ext4 /dev/mmcblk0p3 rw\n"
        );

        let result = check_existing_mount(mountinfo, "/dev/mmcblk0p3", "/data", "ext4");

        assert!(result.is_err());
    }

    #[test]
    fn test_consume_bootloader_request_lf() {
        let before = "active=V2\ncandidate=V1\nmode=bootloader\n";
        let expected = "active=V2\ncandidate=V1\nmode=application\n";

        let result = consume_bootloader_request(before).unwrap();

        assert_eq!(result.as_deref(), Some(expected));
    }

    #[test]
    fn test_consume_bootloader_request() {
        let before = "note=mode=bootloader\r\nactive=V2\r\ncandidate=V1\r\nmode=bootloader\r\nextra=keep";
        let expected = "note=mode=bootloader\r\nactive=V2\r\ncandidate=V1\r\nmode=application\r\nextra=keep";

        let result = consume_bootloader_request(before).unwrap();

        assert_eq!(result.as_deref(), Some(expected));
    }

    #[test]
    fn test_no_bootloader_request() {
        let state = "active=V1\ncandidate=none\nmode=application\n";

        let result = consume_bootloader_request(state).unwrap();

        assert_eq!(result, None);
    }

    #[test]
    fn test_consume_bootloader_request_rejects_invalid_state() {
        for state in [
            "",
            "active=V2\nmode=bootloader\n",
            "active=V3\ncandidate=none\nmode=bootloader\n",
            "active=V2\ncandidate=V3\nmode=bootloader\n",
            "active=V2\ncandidate=V2\nmode=bootloader\n",
            "active=V2\ncandidate=none\n",
            "active=V2\ncandidate=none\nmode=recovery\n",
            "active=V2\nactive=V1\ncandidate=none\nmode=bootloader\n",
            "active=V2\ncandidate=none\ncandidate=V1\nmode=bootloader\n",
            "active=V2\ncandidate=none\nmode=bootloader\nmode=application\n",
        ] {
            let error = consume_bootloader_request(state).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "{state:?}");
        }
    }

    #[cfg(target_os = "linux")]
    mod handoff {
        use super::*;
        use std::{fs, io, path::PathBuf};

        const ORIGINAL: &str = "active=V1\ncandidate=V2\nmode=bootloader\n";
        const EXPECTED: &str = "active=V1\ncandidate=V2\nmode=application\n";

        fn test_path(name: &str) -> PathBuf {
            std::env::temp_dir().join(format!("flxc-handoff-{}-{name}", std::process::id()))
        }

        fn write_error() -> io::Result<()> {
            Err(io::Error::other("simulated persistence failure"))
        }

        #[test]
        fn normal_startup_does_not_write() {
            let path = test_path("normal");
            fs::write(&path, EXPECTED).unwrap();
            let outcome = handle_bootloader_handoff(&path, |_, _| panic!("unexpected write"));
            assert!(matches!(outcome, HandoffOutcome::ContinueNormalStartup(state)
                if state.active == AppSlot::V1 && state.candidate == Some(AppSlot::V2)));
            fs::remove_file(path).unwrap();
        }

        #[test]
        fn successful_handoff() {
            let path = test_path("success");
            fs::write(&path, ORIGINAL).unwrap();
            let outcome = handle_bootloader_handoff(&path, persist_consumed_request);
            assert!(matches!(outcome, HandoffOutcome::StartBootloader));
            assert_eq!(fs::read_to_string(&path).unwrap(), EXPECTED);
            fs::remove_file(path).unwrap();
        }

        #[test]
        fn visible_handoff_after_error_does_not_retry() {
            let path = test_path("visible");
            fs::write(&path, ORIGINAL).unwrap();
            let mut writes = 0;
            let outcome = handle_bootloader_handoff(&path, |path, contents| {
                writes += 1;
                persist_consumed_request(path, contents).unwrap();
                write_error()
            });
            assert!(matches!(outcome, HandoffOutcome::StartBootloader));
            assert_eq!(writes, 1);
            assert_eq!(fs::read_to_string(&path).unwrap(), EXPECTED);
            fs::remove_file(path).unwrap();
        }

        #[test]
        fn verified_cancellation_restarts_original_slot() {
            for active in [AppSlot::V1, AppSlot::V2] {
                let path = test_path(active.as_str());
                let candidate = if active == AppSlot::V1 { "V2" } else { "V1" };
                let fields = format!("active={}\ncandidate={candidate}\nextra=keep\n", active.as_str());
                fs::write(&path, format!("{fields}mode=bootloader\n")).unwrap();
                let mut writes = 0;
                let outcome = handle_bootloader_handoff(&path, |path, contents| {
                    writes += 1;
                    if writes == 1 {
                        return write_error();
                    }
                    persist_consumed_request(path, contents)
                });
                assert!(matches!(outcome, HandoffOutcome::RestartActive(slot) if slot == active));
                assert_eq!(writes, 2);
                assert_eq!(fs::read_to_string(&path).unwrap(), format!("{fields}mode=application\n"));
                fs::remove_file(path).unwrap();
            }
        }

        #[test]
        fn failed_or_unverified_cancellation_is_unsafe() {
            let path = test_path("failed-cancel");
            for failure in 0..3 {
                fs::write(&path, ORIGINAL).unwrap();
                let mut writes = 0;
                let outcome = handle_bootloader_handoff(&path, |path, contents| {
                    writes += 1;
                    match (writes, failure) {
                        (2, 1) => Ok(()), // Writer reports success but state did not change.
                        (2, 2) => {
                            persist_consumed_request(path, contents).unwrap();
                            write_error()
                        }
                        _ => write_error(),
                    }
                });
                assert!(matches!(outcome, HandoffOutcome::Unsafe));
                assert_eq!(writes, 2);
            }
            fs::remove_file(path).unwrap();
        }

        #[test]
        fn unexpected_read_back_does_not_retry() {
            let path = test_path("unexpected");
            for unreadable in [false, true] {
                fs::write(&path, ORIGINAL).unwrap();
                let mut writes = 0;
                let outcome = handle_bootloader_handoff(&path, |path, _| {
                    writes += 1;
                    if unreadable {
                        fs::remove_file(path).unwrap();
                    } else {
                        fs::write(path, "active=INVALID\n").unwrap();
                    }
                    write_error()
                });
                assert!(matches!(outcome, HandoffOutcome::Unsafe));
                assert_eq!(writes, 1);
            }
        }

        #[test]
        fn missing_unreadable_or_malformed_state_does_not_write() {
            let path = test_path("invalid");
            let _ = fs::remove_file(&path);
            let check = || {
                assert!(matches!(
                    handle_bootloader_handoff(&path, |_, _| panic!("unexpected write")),
                    HandoffOutcome::Unsafe
                ));
            };
            check();
            fs::write(&path, [0xff]).unwrap();
            check();
            fs::write(&path, "candidate=none\nmode=bootloader\n").unwrap();
            check();
            fs::remove_file(path).unwrap();
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_persist_consumed_request() {
        use std::fs;
        use std::time::{SystemTime, UNIX_EPOCH};

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("flxc1000-handoff-{}-{unique}", std::process::id()));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("boot_state");
        let original = "active=V2\ncandidate=V1\nmode=bootloader\nextra=keep\n";
        fs::write(&path, original).unwrap();

        let updated = consume_bootloader_request(original)
            .unwrap()
            .expect("bootloader request should be present");
        persist_consumed_request(&path, &updated).unwrap();

        let saved = fs::read_to_string(&path).unwrap();
        assert_eq!(saved, updated);
        assert!(saved.contains("mode=application"));
        assert!(!path.with_extension("handoff.tmp").exists());
        fs::remove_dir_all(&directory).unwrap();
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
