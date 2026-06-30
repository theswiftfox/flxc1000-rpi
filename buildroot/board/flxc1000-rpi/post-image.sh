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

# Post-image script: generates the SD card image using genimage.
# Called by Buildroot after filesystem images are created.
#
# Environment variables provided by Buildroot:
#   BINARIES_DIR  — path to output/images
#   BR2_EXTERNAL_FLXC1000_RPI_PATH — our external tree
#
# Arguments (via BR2_ROOTFS_POST_SCRIPT_ARGS):
#   --board rpi3|rpi4   Select board variant (default: rpi4)
#
set -euo pipefail

BOARD_DIR="$(dirname "$0")"
BOARD=""

while [ $# -gt 0 ]; do
    case "$1" in
        --board) BOARD="$2"; shift 2 ;;
        *) shift ;;
    esac
done

# Select board-specific files (default: existing RPi4 files)
if [ -n "${BOARD}" ]; then
    CONFIG_FILE="config-${BOARD}.txt"
    GENIMAGE_CFG="genimage-${BOARD}.cfg"
else
    CONFIG_FILE="config.txt"
    GENIMAGE_CFG="genimage.cfg"
fi

# Resolve workspace (same path logic as post-build.sh)
WORKSPACE_DIR="$(cd "${BOARD_DIR}/../../.." && pwd)"
RUST_RELEASE="${WORKSPACE_DIR}/target/aarch64-unknown-linux-musl/release"

# Copy firmware files to BINARIES_DIR root so genimage places them at the
# FAT partition root (the Pi firmware expects bootcode.bin, start.elf,
# config.txt etc. at the root, not in a subdirectory).
cp "${BOARD_DIR}/${CONFIG_FILE}" "${BINARIES_DIR}/config.txt"

CMDLINE_FILE="${BOARD_DIR}/cmdline-${BOARD}.txt"
if [ -f "${CMDLINE_FILE}" ]; then
    cp "${CMDLINE_FILE}" "${BINARIES_DIR}/cmdline.txt"
fi

# Copy Buildroot-installed firmware from rpi-firmware/ to root
for f in bootcode.bin start.elf fixup.dat start4.elf fixup4.dat; do
    [ -f "${BINARIES_DIR}/rpi-firmware/${f}" ] && \
        cp "${BINARIES_DIR}/rpi-firmware/${f}" "${BINARIES_DIR}/${f}"
done

# Stage app binary and boot_state for genimage rootpath.
# genimage populates ext4 partitions from mountpoint subdirectories
# within this rootpath, so the SD card image is complete after dd.
ROOTPATH="$(mktemp -d)"
trap 'rm -rf "${ROOTPATH}"' EXIT

mkdir -p "${ROOTPATH}/app" "${ROOTPATH}/data"

if [ -f "${RUST_RELEASE}/flxc1000-app" ]; then
    cp "${RUST_RELEASE}/flxc1000-app" "${ROOTPATH}/app/flxc1000-app"
    chmod 755 "${ROOTPATH}/app/flxc1000-app"
    echo "==> Staged flxc1000-app into app partition"
else
    echo "WARNING: flxc1000-app not found — app partition will be empty"
fi

echo "app_valid" > "${ROOTPATH}/data/boot_state"
echo "==> Staged boot_state (app_valid) into data partition"

# Generate SD card image (call genimage directly — Buildroot's wrapper
# doesn't support a custom rootpath, which we need for mountpoints)
GENIMAGE_TMP="${BUILD_DIR}/genimage.tmp"
rm -rf "${GENIMAGE_TMP}"

genimage \
    --rootpath "${ROOTPATH}" \
    --tmppath "${GENIMAGE_TMP}" \
    --inputpath "${BINARIES_DIR}" \
    --outputpath "${BINARIES_DIR}" \
    --config "${BOARD_DIR}/${GENIMAGE_CFG}"
