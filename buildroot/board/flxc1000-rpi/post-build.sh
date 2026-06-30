#!/bin/bash
# SPDX-License-Identifier: Apache-2.0
# SPDX-FileCopyrightText: 2025 The Contributors to Eclipse OpenSOVD (see CONTRIBUTORS)
#
# See the NOTICE file(s) distributed with this work for additional
# information regarding copyright ownership.
#
# This program and the accompanying materials are made available under the
# terms of the Apache License Version 2.0 which is available at
# https://www.apache.org/licenses/LICENSE-2.0

# Post-build script: copies pre-built Rust binaries into the rootfs staging area.
# Called by Buildroot after all packages are built, before filesystem image creation.
#
# Usage: Buildroot calls this with $1 = TARGET_DIR (rootfs staging dir)
#
# Prerequisites:
#   cargo zigbuild --target aarch64-unknown-linux-musl --release
#
set -euo pipefail

TARGET_DIR="$1"
BOARD_DIR="$(dirname "$0")"
WORKSPACE_DIR="$(cd "${BOARD_DIR}/../../.." && pwd)"
RUST_RELEASE="${WORKSPACE_DIR}/target/aarch64-unknown-linux-musl/release"

echo "==> FLXC1000 post-build: installing Rust binaries from ${RUST_RELEASE}"

# Verify binaries exist
for bin in flxc1000-init flxc1000-boot; do
    if [ ! -f "${RUST_RELEASE}/${bin}" ]; then
        echo "ERROR: ${RUST_RELEASE}/${bin} not found."
        echo "       Run: cargo zigbuild --target aarch64-unknown-linux-musl --release"
        exit 1
    fi
done

# /init — PID 1 init process (from initramfs root)
install -m 0755 "${RUST_RELEASE}/flxc1000-init" "${TARGET_DIR}/init"

# /boot/flxc1000-boot — bootloader UDS handler
mkdir -p "${TARGET_DIR}/boot"
install -m 0755 "${RUST_RELEASE}/flxc1000-boot" "${TARGET_DIR}/boot/flxc1000-boot"

# /app and /data mount points
mkdir -p "${TARGET_DIR}/app"
mkdir -p "${TARGET_DIR}/data"

# /proc /sys /dev (init mounts these, but dirs must exist)
mkdir -p "${TARGET_DIR}/proc"
mkdir -p "${TARGET_DIR}/sys"
mkdir -p "${TARGET_DIR}/dev"

echo "==> FLXC1000 post-build: done"
