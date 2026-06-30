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

# Flash the FLXC1000 SD card image to a block device.
#
# Usage:
#   ./scripts/flash-sdcard.sh /dev/diskN
#   ./scripts/flash-sdcard.sh /dev/sdX        (Linux)
#
# The script expects the sdcard.img to have been produced by a Buildroot build.
# It will also install the flxc1000-app binary onto partition 2 (app).
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

# Default image path (Buildroot output)
BUILDROOT_OUTPUT="${WORKSPACE_DIR}/buildroot-build/images"
SDCARD_IMG="${BUILDROOT_OUTPUT}/sdcard.img"

usage() {
    echo "Usage: $0 <block-device>"
    echo ""
    echo "  block-device   e.g. /dev/disk4 (macOS) or /dev/sdb (Linux)"
    echo ""
    echo "Environment:"
    echo "  SDCARD_IMG     Override path to sdcard.img  (default: ${SDCARD_IMG})"
    echo ""
    echo "The SD card image includes flxc1000-app and boot_state (baked in during build)."
    exit 1
}

if [ $# -ne 1 ]; then
    usage
fi

DEVICE="$1"

# Safety checks
if [ ! -b "${DEVICE}" ] && [ ! -c "${DEVICE}" ]; then
    echo "ERROR: ${DEVICE} is not a block/character device"
    exit 1
fi

if [ ! -f "${SDCARD_IMG}" ]; then
    echo "ERROR: SD card image not found: ${SDCARD_IMG}"
    echo "       Run the Buildroot build first, or set SDCARD_IMG env var."
    exit 1
fi


echo "==> FLXC1000 SD Card Flasher"
echo "    Image:  ${SDCARD_IMG}"
echo "    Device: ${DEVICE}"
echo ""
echo "WARNING: This will ERASE ALL DATA on ${DEVICE}!"
read -rp "Continue? [y/N] " confirm
if [ "${confirm}" != "y" ] && [ "${confirm}" != "Y" ]; then
    echo "Aborted."
    exit 0
fi

# Determine platform-specific details
OS="$(uname -s)"
case "${OS}" in
    Darwin)
        # macOS: unmount all partitions, use rdisk for raw access
        echo "==> Unmounting ${DEVICE} partitions..."
        diskutil unmountDisk "${DEVICE}" 2>/dev/null || true
        if [[ "${DEVICE}" == */rdisk* ]]; then
            RAW_DEVICE="${DEVICE}"
        else
            RAW_DEVICE="${DEVICE/disk/rdisk}"
        fi
        DD_FLAGS="bs=4m"
        ;;
    Linux)
        RAW_DEVICE="${DEVICE}"
        DD_FLAGS="bs=4M status=progress"
        ;;
    *)
        echo "ERROR: Unsupported OS: ${OS}"
        exit 1
        ;;
esac

# Write image (includes app binary and boot_state baked in by genimage)
echo "==> Writing sdcard.img to ${RAW_DEVICE}..."
sudo dd if="${SDCARD_IMG}" of="${RAW_DEVICE}" ${DD_FLAGS}
sync

case "${OS}" in
    Darwin)
        sleep 1
        diskutil eject "${DEVICE}" 2>/dev/null || true
        ;;
esac

echo ""
echo "==> Done! SD card is ready."
echo "    The image includes flxc1000-app and boot_state (app_valid)."
echo "    Insert into RPi and power on."
echo "    DoIP server will listen on port 13400."
