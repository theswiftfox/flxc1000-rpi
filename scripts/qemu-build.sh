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

# Build a QEMU-bootable system image for the FLXC1000 ECU simulator.
#
# Produces:
#   qemu-build/Image          — aarch64 Linux kernel (QEMU virt machine)
#   qemu-build/rootfs.cpio.gz — initramfs with flxc1000 binaries
#
# The kernel is extracted once from Alpine Linux (linux-virt) and cached.
# The initramfs is rebuilt every invocation from cross-compiled Rust binaries.
#
# After building:
#   ./scripts/qemu-run.sh system
#
# Note: In QEMU there are no real block devices (/dev/mmcblk0p*), so
# flxc1000-init cannot mount /app and /data partitions. It falls back to
# the bootloader variant. Use './scripts/qemu-run.sh boot' or 'app' for
# direct user-mode testing of individual variants.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
TARGET_DIR="${WORKSPACE_DIR}/target/aarch64-unknown-linux-musl/release"
OUTPUT_DIR="${WORKSPACE_DIR}/qemu-build"

# ---------------------------------------------------------------------------
# Arguments
# ---------------------------------------------------------------------------
SKIP_BUILD=false
CLEAN=false

while [ $# -gt 0 ]; do
    case "$1" in
        --no-build) SKIP_BUILD=true; shift ;;
        --clean)    CLEAN=true; shift ;;
        -h|--help)
            cat <<EOF
Usage: $0 [options]

Options:
  --no-build   Skip cargo zigbuild (reuse existing binaries in target/)
  --clean      Remove qemu-build/ before building
  -h, --help   Show this help

Prerequisites:
  cargo-zigbuild   cargo install cargo-zigbuild
  Docker           Required on macOS for kernel extraction
  cpio, gzip       Standard system tools

Output:
  qemu-build/Image            aarch64 kernel for QEMU virt
  qemu-build/rootfs.cpio.gz   initramfs (init + boot + app binaries)

Run the image:
  ./scripts/qemu-run.sh system
EOF
            exit 0
            ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [ "${CLEAN}" = true ]; then
    echo "==> Cleaning ${OUTPUT_DIR}/"
    rm -rf "${OUTPUT_DIR}"
fi

mkdir -p "${OUTPUT_DIR}"

# ---------------------------------------------------------------------------
# Step 1: Cross-compile Rust binaries
# ---------------------------------------------------------------------------
if [ "${SKIP_BUILD}" = false ]; then
    echo "==> Cross-compiling Rust binaries (aarch64-unknown-linux-musl)"
    cargo zigbuild \
        --target aarch64-unknown-linux-musl \
        --release \
        -p flxc1000-init \
        -p flxc1000-boot \
        -p flxc1000-app \
        --manifest-path "${WORKSPACE_DIR}/Cargo.toml"
else
    echo "==> Skipping cargo build (--no-build)"
fi

for bin in flxc1000-init flxc1000-boot; do
    if [ ! -f "${TARGET_DIR}/${bin}" ]; then
        echo "ERROR: ${TARGET_DIR}/${bin} not found."
        echo "       Run without --no-build, or build manually:"
        echo "         cargo zigbuild --target aarch64-unknown-linux-musl --release"
        exit 1
    fi
done

# ---------------------------------------------------------------------------
# Step 2: Assemble initramfs
# ---------------------------------------------------------------------------
echo "==> Assembling initramfs"
STAGING="${OUTPUT_DIR}/.initramfs-staging"
rm -rf "${STAGING}"
mkdir -p "${STAGING}"/{boot,app,data,proc,sys,dev,tmp}

install -m 755 "${TARGET_DIR}/flxc1000-init" "${STAGING}/init"
install -m 755 "${TARGET_DIR}/flxc1000-boot" "${STAGING}/boot/flxc1000-boot"

if [ -f "${TARGET_DIR}/flxc1000-app" ]; then
    install -m 755 "${TARGET_DIR}/flxc1000-app" "${STAGING}/app/flxc1000-app"
fi

(cd "${STAGING}" && find . | sort | cpio -o -H newc 2>/dev/null) \
    | gzip > "${OUTPUT_DIR}/rootfs.cpio.gz"

rm -rf "${STAGING}"

CPIO_SIZE="$(du -h "${OUTPUT_DIR}/rootfs.cpio.gz" | cut -f1 | tr -d '[:space:]')"
echo "    rootfs.cpio.gz  ${CPIO_SIZE}"

# ---------------------------------------------------------------------------
# Step 3: Fetch kernel (cached)
# ---------------------------------------------------------------------------
if [ -f "${OUTPUT_DIR}/Image" ]; then
    KERNEL_SIZE="$(du -h "${OUTPUT_DIR}/Image" | cut -f1 | tr -d '[:space:]')"
    echo "==> Kernel cached (${KERNEL_SIZE}): ${OUTPUT_DIR}/Image"
else
    echo "==> Fetching aarch64 virt kernel from Alpine Linux (one-time)"

    if ! command -v docker &>/dev/null; then
        echo "ERROR: Docker is required to fetch the kernel."
        echo "       Install Docker Desktop, or manually place an aarch64 Image at:"
        echo "         ${OUTPUT_DIR}/Image"
        exit 1
    fi

    docker run --rm --platform linux/arm64 alpine:latest sh -c \
        'apk add --no-cache linux-virt >/dev/null 2>&1 && cat /boot/vmlinuz-virt' \
        > "${OUTPUT_DIR}/Image.tmp"

    # Sanity check — kernel should be at least 1 MiB
    KERNEL_BYTES="$(wc -c < "${OUTPUT_DIR}/Image.tmp" | tr -d '[:space:]')"
    if [ "${KERNEL_BYTES}" -lt 1048576 ]; then
        rm -f "${OUTPUT_DIR}/Image.tmp"
        echo "ERROR: Kernel fetch produced only ${KERNEL_BYTES} bytes."
        echo "       Check Docker and network connectivity."
        exit 1
    fi

    mv "${OUTPUT_DIR}/Image.tmp" "${OUTPUT_DIR}/Image"
    KERNEL_SIZE="$(du -h "${OUTPUT_DIR}/Image" | cut -f1 | tr -d '[:space:]')"
    echo "    Image           ${KERNEL_SIZE}"
fi

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
echo ""
echo "==> QEMU system image ready"
echo "    ${OUTPUT_DIR}/Image"
echo "    ${OUTPUT_DIR}/rootfs.cpio.gz"
echo ""
echo "    Boot with:"
echo "      ./scripts/qemu-run.sh system"
