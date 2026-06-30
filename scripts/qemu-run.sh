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

# Run FLXC1000 binaries in QEMU for development/testing.
#
# Modes:
#   boot     — Run flxc1000-boot directly (fastest, no kernel needed)
#   app      — Run flxc1000-app directly
#   system   — Boot full system image in QEMU system-mode (requires Buildroot output)
#
# How it runs:
#   Linux:  qemu-aarch64 user-mode emulation (native speed for boot/app)
#   macOS:  Docker container with linux/arm64 (native on Apple Silicon, emulated on Intel)
#
# Prerequisites:
#   Linux:  sudo apt install qemu-user-static              (user-mode)
#           sudo apt install qemu-system-arm               (system-mode)
#   macOS:  Docker Desktop (for boot/app user-mode)
#           brew install qemu                              (system-mode only)
#
# Cross-compile first:
#   cargo zigbuild --target aarch64-unknown-linux-musl --release
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKSPACE_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
TARGET_DIR="${WORKSPACE_DIR}/target/aarch64-unknown-linux-musl/release"

# IP address for the virtual ECU on the host.
# Default 127.0.0.2 avoids conflict with CDA binding 127.0.0.1:13400.
# macOS routes all of 127.0.0.0/8 to lo0 by default.
ECU_IP="${ECU_IP:-127.0.0.2}"

usage() {
    cat <<EOF
Usage: $0 <mode> [options]

Modes:
  boot     Run flxc1000-boot directly (no kernel needed)
  app      Run flxc1000-app directly (no kernel needed)
  system   Boot full system image with kernel + initramfs (requires Buildroot)

Options (system mode):
  --kernel PATH    Path to Image  (default: buildroot-build/images/Image)
  --initrd PATH    Path to rootfs.cpio.lz4  (default: buildroot-build/images/rootfs.cpio.lz4)

boot/app mode:
  Linux:  Uses qemu-aarch64 user-mode emulation
  macOS:  Uses Docker (linux/arm64 container — native on Apple Silicon)
  - DoIP listens on ECU_IP:13400 (default: 127.0.0.2:13400)
  - Does NOT conflict with CDA on 127.0.0.1:13400
  - GPIO operations are simulated (no /dev/gpiomem)
  - Network commands (ip link) will fail harmlessly

Environment:
  ECU_IP    IP to bind the virtual ECU on (default: 127.0.0.2)

System mode:
  - Uses qemu-system-aarch64 with 'virt' machine
  - Port 13400 is forwarded: host:13400 -> guest:13400
  - No GPU, no GPIO — headless diagnostic testing only
  - Serial console on stdio

Examples:
  $0 boot                          # Quick test boot variant
  $0 app                           # Quick test app variant
  $0 system                        # Full system boot
  $0 system --kernel /path/Image   # Custom kernel
EOF
    exit 1
}

# ---------------------------------------------------------------------------
# User-mode: run a single binary (Linux: qemu-aarch64, macOS: Docker)
# ---------------------------------------------------------------------------
run_user_mode() {
    local binary="$1"
    local binary_path="${TARGET_DIR}/${binary}"

    if [ ! -f "${binary_path}" ]; then
        echo "ERROR: ${binary_path} not found."
        echo "       Build first: cargo zigbuild --target aarch64-unknown-linux-musl --release -p ${binary}"
        exit 1
    fi

    local os="$(uname -s)"

    if [ "${os}" = "Linux" ]; then
        run_user_mode_linux "${binary}" "${binary_path}"
    elif [ "${os}" = "Darwin" ]; then
        run_user_mode_docker "${binary}" "${binary_path}"
    else
        echo "ERROR: Unsupported OS: ${os}"
        exit 1
    fi
}

run_user_mode_linux() {
    local binary="$1"
    local binary_path="$2"

    # Find QEMU user-mode binary
    local qemu=""
    for candidate in qemu-aarch64 qemu-aarch64-static; do
        if command -v "${candidate}" &>/dev/null; then
            qemu="${candidate}"
            break
        fi
    done

    if [ -z "${qemu}" ]; then
        echo "ERROR: qemu-aarch64 not found in PATH."
        echo "       Install: sudo apt install qemu-user-static"
        exit 1
    fi

    echo "==> Running ${binary} via ${qemu} (user-mode)"
    echo "    DoIP will listen on 0.0.0.0:13400"
    echo "    Press Ctrl+C to stop"
    echo ""

    # Static musl binary — no sysroot needed
    exec "${qemu}" "${binary_path}"
}

run_user_mode_docker() {
    local binary="$1"
    local binary_path="$2"

    if ! command -v docker &>/dev/null; then
        echo "ERROR: Docker not found."
        echo "       On macOS, qemu-aarch64 (user-mode) is not available."
        echo "       Install Docker Desktop: https://www.docker.com/products/docker-desktop/"
        echo ""
        echo "       Alternatively, use system-mode with a Buildroot kernel:"
        echo "         $0 system"
        exit 1
    fi

    # macOS only routes 127.0.0.0/8 but doesn't bind arbitrary addresses.
    # We need a loopback alias so Docker can publish ports on ECU_IP.
    if [ "${ECU_IP}" != "127.0.0.1" ]; then
        if ! ifconfig lo0 | grep -q "inet ${ECU_IP} "; then
            echo "==> Adding loopback alias ${ECU_IP} (requires sudo, persists until reboot)"
            sudo ifconfig lo0 alias "${ECU_IP}"
        fi
    fi

    # UDP broadcasts don't reach loopback / Docker port-forwards.
    # Route UDP through an internal port and relay broadcasts from the host.
    local relay_port=13401

    python3 "${SCRIPT_DIR}/doip-udp-relay.py" \
        --ecu-ip "${ECU_IP}" \
        --internal-port "${relay_port}" &
    local relay_pid=$!
    trap "kill ${relay_pid} 2>/dev/null" EXIT

    echo "==> Running ${binary} via Docker (linux/arm64)"
    echo "    DoIP TCP : ${ECU_IP}:13400  (direct)"
    echo "    DoIP UDP : ${ECU_IP}:13400  (via broadcast relay, pid ${relay_pid})"
    echo "    CDA can coexist on 127.0.0.1:13400"
    echo "    Press Ctrl+C to stop"
    echo ""

    docker run --rm -it \
        --platform linux/arm64 \
        -p "${ECU_IP}:13400:13400/tcp" \
        -p "127.0.0.1:${relay_port}:13400/udp" \
        -v "${binary_path}:/usr/local/bin/${binary}:ro" \
        alpine:latest \
        "/usr/local/bin/${binary}"
}

# ---------------------------------------------------------------------------
# System-mode emulation
# ---------------------------------------------------------------------------
run_system_mode() {
    # Default paths: prefer qemu-build/ output, fall back to Buildroot output
    local kernel="${WORKSPACE_DIR}/qemu-build/Image"
    local initrd="${WORKSPACE_DIR}/qemu-build/rootfs.cpio.gz"
    if [ ! -f "${kernel}" ]; then
        kernel="${WORKSPACE_DIR}/buildroot-build/images/Image"
    fi
    if [ ! -f "${initrd}" ]; then
        initrd="${WORKSPACE_DIR}/buildroot-build/images/rootfs.cpio.lz4"
    fi

    # Parse options (overrides above)
    while [ $# -gt 0 ]; do
        case "$1" in
            --kernel) kernel="$2"; shift 2 ;;
            --initrd) initrd="$2"; shift 2 ;;
            *) echo "Unknown option: $1"; usage ;;
        esac
    done

    if [ ! -f "${kernel}" ]; then
        echo "ERROR: Kernel not found: ${kernel}"
        echo "       Build with: ./scripts/qemu-build.sh"
        echo "       Or specify: --kernel PATH"
        exit 1
    fi

    if [ ! -f "${initrd}" ]; then
        echo "ERROR: Initrd not found: ${initrd}"
        echo "       Build with: ./scripts/qemu-build.sh"
        echo "       Or specify: --initrd PATH"
        exit 1
    fi

    local qemu=""
    for candidate in qemu-system-aarch64; do
        if command -v "${candidate}" &>/dev/null; then
            qemu="${candidate}"
            break
        fi
    done

    if [ -z "${qemu}" ]; then
        echo "ERROR: qemu-system-aarch64 not found in PATH."
        echo "       Install QEMU:"
        echo "         macOS:  brew install qemu"
        echo "         Linux:  sudo apt install qemu-system-arm"
        exit 1
    fi

    echo "==> Booting FLXC1000 in QEMU system mode"
    echo "    Kernel:  ${kernel}"
    echo "    Initrd:  ${initrd}"
    echo "    Machine: virt (aarch64)"
    echo "    Network: host:13400 -> guest:13400 (DoIP)"
    echo "    Console: serial on stdio"
    echo ""
    echo "    Press Ctrl-A X to exit QEMU"
    echo ""

    exec "${qemu}" \
        -machine virt \
        -cpu cortex-a72 \
        -m 512M \
        -nographic \
        -kernel "${kernel}" \
        -initrd "${initrd}" \
        -append "console=ttyAMA0 rdinit=/init" \
        -netdev user,id=net0,hostfwd=tcp::13400-:13400,hostfwd=udp::13400-:13400 \
        -device virtio-net-pci,netdev=net0
}

# ---------------------------------------------------------------------------
# Main dispatch
# ---------------------------------------------------------------------------
if [ $# -lt 1 ]; then
    usage
fi

MODE="$1"
shift

case "${MODE}" in
    boot)   run_user_mode "flxc1000-boot" ;;
    app)    run_user_mode "flxc1000-app" ;;
    system) run_system_mode "$@" ;;
    *)      usage ;;
esac
