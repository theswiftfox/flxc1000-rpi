#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# SPDX-FileCopyrightText: 2025 The Contributors to Eclipse OpenSOVD (see CONTRIBUTORS)
#
# See the NOTICE file(s) distributed with this work for additional
# information regarding copyright ownership.
#
# This program and the accompanying materials are made available under the
# terms of the Apache License Version 2.0 which is available at
# https://www.apache.org/licenses/LICENSE-2.0

# Quick connectivity check for the flxc1000-rpi application
# Tests: ping, UDP VIR (Vehicle Identification Request), TCP DoIP port

set -euo pipefail

RPI_IP="${1:-169.254.16.1}"
DOIP_PORT=13400
TIMEOUT=2

RED='\033[0;31m'
GRN='\033[0;32m'
YEL='\033[0;33m'
RST='\033[0m'

ok()   { printf "${GRN}[OK]${RST}   %s\n" "$1"; }
fail() { printf "${RED}[FAIL]${RST} %s\n" "$1"; }
info() { printf "${YEL}[INFO]${RST} %s\n" "$1"; }

echo "=== flxc1000-rpi connectivity check ==="
echo "Target: $RPI_IP:$DOIP_PORT"
echo

# --- 1. Ping ---
info "Ping..."
if ping -c 1 -W "$TIMEOUT" "$RPI_IP" >/dev/null 2>&1; then
    ok "Ping $RPI_IP reachable"
else
    fail "Ping $RPI_IP unreachable — check cable/IP config"
    # exit 1
fi

# --- 2. TCP port check ---
info "TCP connect to $RPI_IP:$DOIP_PORT..."
if nc -z -w "$TIMEOUT" "$RPI_IP" "$DOIP_PORT" 2>/dev/null; then
    ok "TCP port $DOIP_PORT open (DoIP TCP listener is up)"
else
    fail "TCP port $DOIP_PORT closed — app may not be running"
fi

# --- 3. UDP Vehicle Identification Request ---
# DoIP header: ver=0x02, inv=0xFD, type=0x0001 (VIR), len=0x00000000
VIR_HEX="02fd000100000000"

info "Sending UDP VIR (Vehicle Identification Request)..."
RESPONSE=$(printf '%s' "$VIR_HEX" \
    | xxd -r -p \
    | nc -u -w "$TIMEOUT" "$RPI_IP" "$DOIP_PORT" 2>/dev/null \
    | xxd -p -l 64 || true)

if [ -n "$RESPONSE" ]; then
    ok "Got UDP response (VAM): $RESPONSE"
    # Parse VIN from VAM payload: header(8) + VIN starts at offset 8, 17 bytes
    VIN_HEX=$(echo "$RESPONSE" | cut -c17-50)
    VIN=$(echo "$VIN_HEX" | xxd -r -p 2>/dev/null || true)
    if [ -n "$VIN" ]; then
        info "VIN: $VIN"
    fi
    # Parse logical address: 2 bytes after VIN (offset 8+17=25 in payload, byte 33-36 hex chars)
    ADDR_HEX=$(echo "$RESPONSE" | cut -c51-54)
    if [ -n "$ADDR_HEX" ]; then
        info "Logical address: 0x${ADDR_HEX}"
    fi
else
    fail "No UDP VIR response — DoIP UDP handler may not be running"
fi

# --- 4. TesterPresent via TCP (full DoIP handshake) ---
info "Attempting TesterPresent (0x3E) via TCP DoIP..."

# Routing Activation Request: ver=02, inv=FD, type=0005, len=7
# payload: source_addr=0x0E00, activation_type=0x00, reserved=00000000
RA_HEX="02fd00050000000700000e000000000000"

# Diagnostic Message wrapping TesterPresent:
# ver=02, inv=FD, type=8001, len=6
# payload: source_addr=0x0E00, target_addr=0x1000, data=3E00
DIAG_TP_HEX="02fd800100000006000010003e00"

TCP_RESULT=$(
{
    # Send routing activation, wait briefly, then send TesterPresent
    printf '%s' "$RA_HEX" | xxd -r -p
    sleep 0.3
    printf '%s' "$DIAG_TP_HEX" | xxd -r -p
    sleep 0.5
} | nc -w "$TIMEOUT" "$RPI_IP" "$DOIP_PORT" 2>/dev/null | xxd -p -l 128 || true
)

if [ -n "$TCP_RESULT" ]; then
    ok "TCP DoIP response received: $TCP_RESULT"
    # Look for positive TesterPresent response (7E 00) in the stream
    if echo "$TCP_RESULT" | grep -q "7e00"; then
        ok "TesterPresent positive response (7E 00) — UDS stack is alive!"
    elif echo "$TCP_RESULT" | grep -q "7f3e"; then
        fail "TesterPresent negative response — check UDS config"
    else
        info "Got TCP data but couldn't identify TesterPresent response in stream"
    fi
else
    fail "No TCP DoIP response — app may have crashed after boot"
fi

echo
echo "=== Done ==="
