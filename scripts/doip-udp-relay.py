#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
# SPDX-FileCopyrightText: 2025 The Contributors to Eclipse OpenSOVD (see CONTRIBUTORS)
#
# See the NOTICE file(s) distributed with this work for additional
# information regarding copyright ownership.
#
# This program and the accompanying materials are made available under the
# terms of the Apache License Version 2.0 which is available at
# https://www.apache.org/licenses/LICENSE-2.0

"""
DoIP UDP broadcast relay for local development.

Problem: UDP broadcasts don't reach loopback interfaces, so a DoIP ECU
in Docker can't receive Vehicle Identification Requests (VIR) sent as
broadcasts by the CDA.

Solution: This relay catches VIR broadcasts on the host, forwards them
as unicast to the ECU's Docker-published internal UDP port, and relays
the Vehicle Announcement Message (VAM) response back to the CDA with
source address ECU_IP:13400 — so the CDA connects TCP to the right place.

    CDA --broadcast VIR--> relay (0.0.0.0:13400)
                              |
                              v  unicast
                           127.0.0.1:INTERNAL_PORT (Docker -> container:13400)
                              |
                              v  VAM response
    CDA <-- VAM from ECU_IP:13400 (relay)
"""

import argparse
import select
import socket
import sys


def main():
    p = argparse.ArgumentParser(description="DoIP UDP broadcast relay")
    p.add_argument("--ecu-ip", default="127.0.0.2",
                   help="IP the ECU should appear at (default: 127.0.0.2)")
    p.add_argument("--internal-port", type=int, default=13401,
                   help="Docker-published internal UDP port (default: 13401)")
    p.add_argument("--doip-port", type=int, default=13400,
                   help="Standard DoIP UDP port (default: 13400)")
    args = p.parse_args()

    def make_socket():
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        if hasattr(socket, "SO_REUSEPORT"):
            s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEPORT, 1)
        return s

    # rx: catch broadcasts (and unicast) on all interfaces
    rx = make_socket()
    rx.bind(("", args.doip_port))

    # tx: send replies that appear to originate from ECU_IP:13400
    tx = make_socket()
    tx.bind((args.ecu_ip, args.doip_port))

    # fwd: talk to the ECU container via Docker's internal port forward
    fwd = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    fwd.settimeout(2.0)

    ecu_addr = ("127.0.0.1", args.internal_port)
    ignore = frozenset((args.ecu_ip, "127.0.0.1"))

    print(f"doip-udp-relay: 0.0.0.0:{args.doip_port} -> "
          f"{ecu_addr[0]}:{ecu_addr[1]}, "
          f"replies from {args.ecu_ip}:{args.doip_port}",
          file=sys.stderr, flush=True)

    # Select on both rx and tx to handle SO_REUSEPORT packet distribution
    while True:
        ready, _, _ = select.select([rx, tx], [], [], 1.0)

        for sock in ready:
            data, sender = sock.recvfrom(4096)

            if sender[0] in ignore:
                continue

            fwd.sendto(data, ecu_addr)

            try:
                resp, _ = fwd.recvfrom(4096)
            except socket.timeout:
                print(f"  no ECU response for {sender}", file=sys.stderr, flush=True)
                continue

            tx.sendto(resp, sender)


if __name__ == "__main__":
    main()
