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

#[cfg(target_os = "linux")]
fn main() {
    use std::ffi::CString;
    use std::fs;

    use nix::mount::{mount, MsFlags};
    use nix::unistd::execve;

    const DATA_PARTITION: &str = "/dev/mmcblk0p3";
    const DATA_MOUNT: &str = "/data";
    const APP_PARTITION: &str = "/dev/mmcblk0p2";
    const APP_MOUNT: &str = "/app";
    const BOOT_STATE_PATH: &str = "/data/boot_state";
    const APP_BINARY: &str = "/app/flxc1000-app";
    const BOOT_BINARY: &str = "/boot/flxc1000-boot";

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

    if let Err(e) = mount_partition(DATA_PARTITION, DATA_MOUNT, "ext4") {
        eprintln!("[init] Failed to mount /data: {} — falling back to boot", e);
        exec_binary(BOOT_BINARY);
    }

    if let Err(e) = mount_partition(APP_PARTITION, APP_MOUNT, "ext4") {
        eprintln!("[init] Failed to mount /app: {} — falling back to boot", e);
        exec_binary(BOOT_BINARY);
    }

    let boot_state = fs::read_to_string(BOOT_STATE_PATH)
        .unwrap_or_default()
        .trim()
        .to_string();

    eprintln!("[init] boot_state = {:?}", boot_state);

    if boot_state == "app_valid" && std::path::Path::new(APP_BINARY).exists() {
        eprintln!("[init] Executing application: {}", APP_BINARY);
        exec_binary(APP_BINARY);
    } else {
        eprintln!("[init] Executing bootloader: {}", BOOT_BINARY);
        exec_binary(BOOT_BINARY);
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("flxc1000-init is Linux-only. Cross-compile with:");
    eprintln!("  cargo zigbuild --target aarch64-unknown-linux-musl --release -p flxc1000-init");
    std::process::exit(1);
}
