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

# Build the FLXC1000 Raspberry Pi SD card image.
#
# Supports:
#   --board rpi3   Raspberry Pi 3 B+
#   --board rpi4   Raspberry Pi 4 (default)
#
# Produces:
#   buildroot-build/images/sdcard.img        — SD card image
#   buildroot-build/images/Image             — kernel
#   buildroot-build/images/rootfs.cpio.lz4   — initramfs
#
# On Linux:  Buildroot runs natively.
# On macOS:  Buildroot runs in a Docker container with a case-sensitive
#            volume (macOS default APFS is case-insensitive, which breaks
#            some Buildroot packages).
#
# After building:
#   ./scripts/flash-sdcard.sh /dev/diskN
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
TARGET_DIR="${WORKSPACE_DIR}/target/aarch64-unknown-linux-musl/release"

BUILDROOT_VERSION="${BUILDROOT_VERSION:-2024.02.10}"
BUILDROOT_URL="https://buildroot.org/downloads/buildroot-${BUILDROOT_VERSION}.tar.xz"
BUILDROOT_SRC_DIR="${WORKSPACE_DIR}/buildroot-src"
BUILDROOT_SRC="${BUILDROOT_SRC_DIR}/buildroot-${BUILDROOT_VERSION}"
BUILDROOT_OUTPUT="${WORKSPACE_DIR}/buildroot-build"

BOARD="rpi4"
SKIP_CARGO=false
CLEAN=false
JOBS=""

# ---------------------------------------------------------------------------
# Usage
# ---------------------------------------------------------------------------
usage() {
    cat <<EOF
Usage: $0 [options]

Options:
  --board BOARD  Board variant: rpi3 or rpi4 (default: rpi4)
  --no-build     Skip cargo zigbuild (reuse existing Rust binaries)
  --clean        Remove buildroot-build/ before building
  --jobs N       Parallel make jobs (default: nproc)
  -h, --help     Show this help

Environment:
  BUILDROOT_VERSION   Buildroot release (default: ${BUILDROOT_VERSION})
  JOBS                Parallel make jobs

Prerequisites:
  cargo-zigbuild      cargo install cargo-zigbuild
  Linux native:       build-essential libncurses-dev libssl-dev bc cpio
                      rsync unzip wget file perl python3
  macOS:              Docker Desktop

Output:
  buildroot-build/images/sdcard.img         Flash with flash-sdcard.sh
  buildroot-build/images/Image              Kernel
  buildroot-build/images/rootfs.cpio.lz4    Initramfs
EOF
    exit 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --board)    BOARD="$2"; shift 2 ;;
        --no-build) SKIP_CARGO=true; shift ;;
        --clean)    CLEAN=true; shift ;;
        --jobs)     JOBS="$2"; shift 2 ;;
        -h|--help)  usage ;;
        *)          echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [ -z "${JOBS}" ]; then
    JOBS="$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)"
fi

DEFCONFIG="flxc1000_${BOARD}_defconfig"

case "${BOARD}" in
    rpi3|rpi4) ;;
    *) echo "ERROR: Unknown board '${BOARD}'. Use rpi3 or rpi4."; exit 1 ;;
esac

echo "==> Board: ${BOARD}  defconfig: ${DEFCONFIG}"

if [ "${CLEAN}" = true ]; then
    echo "==> Cleaning buildroot-build/"
    rm -rf "${BUILDROOT_OUTPUT}"
    if [ "$(uname -s)" = "Darwin" ] && command -v docker &>/dev/null; then
        docker volume rm "flxc1000-buildroot-${BOARD}" 2>/dev/null && \
            echo "    Removed Docker volume flxc1000-buildroot-${BOARD}" || true
    fi
fi

# ---------------------------------------------------------------------------
# Step 1: Cross-compile Rust binaries (runs on host, both macOS and Linux)
# ---------------------------------------------------------------------------
if [ "${SKIP_CARGO}" = false ]; then
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
# Step 2: Download Buildroot source (host — tarball is fine on any FS)
# ---------------------------------------------------------------------------
if [ ! -d "${BUILDROOT_SRC}" ]; then
    echo "==> Downloading Buildroot ${BUILDROOT_VERSION}"
    mkdir -p "${BUILDROOT_SRC_DIR}"
    curl -L --progress-bar "${BUILDROOT_URL}" \
        | tar -xJ -C "${BUILDROOT_SRC_DIR}"

    if [ ! -d "${BUILDROOT_SRC}" ]; then
        echo "ERROR: Expected ${BUILDROOT_SRC} after extraction."
        echo "       Check BUILDROOT_VERSION=${BUILDROOT_VERSION}"
        exit 1
    fi
else
    echo "==> Buildroot ${BUILDROOT_VERSION} cached"
fi

# ---------------------------------------------------------------------------
# Step 3: Build
# ---------------------------------------------------------------------------
OS="$(uname -s)"

build_native() {
    echo "==> Configuring Buildroot (${DEFCONFIG})"
    make -C "${BUILDROOT_SRC}" \
        BR2_EXTERNAL="${WORKSPACE_DIR}/buildroot" \
        O="${BUILDROOT_OUTPUT}" \
        "${DEFCONFIG}"

    echo "==> Building (jobs=${JOBS})"
    make -C "${BUILDROOT_OUTPUT}" -j"${JOBS}"
}

build_docker() {
    echo "==> Running Buildroot in Docker (macOS)"

    if ! command -v docker &>/dev/null; then
        echo "ERROR: Docker is required for Buildroot builds on macOS."
        exit 1
    fi

    # Docker volume provides a case-sensitive ext4 filesystem inside the
    # Docker VM, avoiding macOS APFS case-insensitivity issues.
    local vol="flxc1000-buildroot-${BOARD}"
    docker volume create "${vol}" >/dev/null 2>&1 || true

    mkdir -p "${BUILDROOT_OUTPUT}/images"

    # post-build.sh resolves the workspace relative to BOARD_DIR:
    #   BOARD_DIR = <BR2_EXTERNAL>/board/flxc1000-rpi
    #   WORKSPACE = BOARD_DIR/../../..
    # With BR2_EXTERNAL=/work/external, WORKSPACE resolves to /work.
    # We place Rust binaries at /work/target/... so post-build.sh finds them.

    docker run --rm -i \
        -v "${vol}:/work" \
        -v "${WORKSPACE_DIR}/buildroot:/host-external:ro" \
        -v "${TARGET_DIR}:/host-rust-bins:ro" \
        -v "${BUILDROOT_OUTPUT}/images:/output-images" \
        -e BUILDROOT_VERSION="${BUILDROOT_VERSION}" \
        -e DEFCONFIG="${DEFCONFIG}" \
        -e JOBS="${JOBS}" \
        debian:bookworm bash <<'INNER'
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive

echo "==> Installing Buildroot dependencies"
apt-get update -qq >/dev/null
apt-get install -y -qq \
    build-essential bc cpio file rsync unzip wget curl \
    libncurses-dev libssl-dev perl python3 xz-utils \
    dosfstools mtools e2fsprogs >/dev/null 2>&1

BRSRC="/work/buildroot-${BUILDROOT_VERSION}"
BROUT="/work/output"

# Download Buildroot into the volume (cached across runs)
if [ ! -d "${BRSRC}" ]; then
    echo "==> Downloading Buildroot ${BUILDROOT_VERSION} into Docker volume"
    curl -sL "https://buildroot.org/downloads/buildroot-${BUILDROOT_VERSION}.tar.xz" \
        | tar -xJ -C /work
fi

# Sync external tree into the volume
rsync -a --delete /host-external/ /work/external/

# Place Rust binaries where post-build.sh expects them
mkdir -p /work/target/aarch64-unknown-linux-musl/release
for bin in flxc1000-init flxc1000-boot flxc1000-app; do
    [ -f "/host-rust-bins/${bin}" ] && \
        cp "/host-rust-bins/${bin}" /work/target/aarch64-unknown-linux-musl/release/
done

# Configure
echo "==> Configuring Buildroot (${DEFCONFIG})"
make -C "${BRSRC}" \
    BR2_EXTERNAL=/work/external \
    O="${BROUT}" \
    "${DEFCONFIG}"

# If .config changed from previous build, force-clean affected packages
CONFIG_HASH="$(md5sum "${BROUT}/.config" 2>/dev/null | cut -d' ' -f1)"
PREV_HASH=""
[ -f "${BROUT}/.config.md5" ] && PREV_HASH="$(cat "${BROUT}/.config.md5")"
if [ "${CONFIG_HASH}" != "${PREV_HASH}" ] && [ -n "${PREV_HASH}" ]; then
    echo "==> Config changed — cleaning rpi-firmware to pick up variant changes"
    make -C "${BROUT}" rpi-firmware-dirclean 2>/dev/null || true
fi
echo "${CONFIG_HASH}" > "${BROUT}/.config.md5"

# Build
echo "==> Building (jobs=${JOBS})"
make -C "${BROUT}" -j"${JOBS}"

# Copy artifacts to host
cp -a "${BROUT}"/images/* /output-images/
echo "==> Artifacts copied to host"
INNER
}

case "${OS}" in
    Linux)  build_native ;;
    Darwin) build_docker ;;
    *)
        echo "ERROR: Unsupported OS: ${OS}"
        exit 1
        ;;
esac

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
echo ""
echo "==> ${BOARD} build complete"
echo "    Output: ${BUILDROOT_OUTPUT}/images/"

if [ -f "${BUILDROOT_OUTPUT}/images/sdcard.img" ]; then
    IMG_SIZE="$(du -h "${BUILDROOT_OUTPUT}/images/sdcard.img" | cut -f1 | tr -d '[:space:]')"
    echo "    sdcard.img  ${IMG_SIZE}"
fi
if [ -f "${BUILDROOT_OUTPUT}/images/Image" ]; then
    echo "    Image       (kernel)"
fi
if [ -f "${BUILDROOT_OUTPUT}/images/rootfs.cpio.lz4" ]; then
    echo "    rootfs.cpio.lz4  (initramfs)"
fi

echo ""
echo "    Flash to SD card:"
echo "      ./scripts/flash-sdcard.sh /dev/diskN"
echo ""
echo "    Or boot in QEMU (system mode):"
echo "      ./scripts/qemu-run.sh system"
