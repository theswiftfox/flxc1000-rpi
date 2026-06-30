<!--
SPDX-License-Identifier: Apache-2.0
SPDX-FileCopyrightText: 2025 The Contributors to Eclipse OpenSOVD (see CONTRIBUTORS)

See the NOTICE file(s) distributed with this work for additional
information regarding copyright ownership.

This program and the accompanying materials are made available under the
terms of the Apache License Version 2.0 which is available at
https://www.apache.org/licenses/LICENSE-2.0
-->

# FLXC1000-RPI — ECU Simulator for Raspberry Pi 4

> **Disclaimer:** The majority of this codebase was generated with the assistance
> of AI tools. While it has been reviewed and tested, please exercise due
> diligence when using or adapting this code.

UDS-over-DoIP ECU simulator running on a Raspberry Pi 4 with a minimal
Buildroot Linux image. Implements ISO 14229 (UDS) via
[ace-server](https://github.com/samp-reston/ace) and ISO 13400 (DoIP) with a
custom async networking layer.

## Architecture

```
┌──────────────────────────────────────────────────────┐
│  SD Card                                             │
│  P1 (boot/FAT32)  kernel + initramfs (CPIO+LZ4)      │
│  P2 (app/ext4)    flxc1000-app binary                │
│  P3 (data/ext4)   boot_state, persistent storage     │
└──────────────────────────────────────────────────────┘

Boot sequence:
  kernel → initramfs → /init (flxc1000-init)
    ├─ boot_state == "app_valid"  →  exec /app/flxc1000-app
    └─ otherwise                  →  exec /boot/flxc1000-boot
```

| Binary | Role | UDS Services |
|--------|------|-------------|
| `flxc1000-init` | PID 1 — mounts partitions, decides boot vs app | — |
| `flxc1000-boot` | Bootloader variant — flash programming | 0x10, 0x11, 0x22, 0x27, 0x34, 0x36, 0x37, 0x3E |
| `flxc1000-app` | Application variant — diagnostics, GPIO | 0x10, 0x11, 0x14, 0x19, 0x22, 0x2E, 0x31, 0x3E |

DoIP listens on **UDP+TCP port 13400**, ECU logical address **0x1000**.

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Rust | ≥ 1.85 | [rustup.rs](https://rustup.rs) |
| Cross-compiler | see below | one of the options below |
| QEMU (optional) | ≥ 8.0 | `brew install qemu` |
| Buildroot (optional) | 2024.02+ | `git clone https://gitlab.com/buildroot.org/buildroot.git` |

Add the musl target:

```bash
rustup target add aarch64-unknown-linux-musl
```

**Cross-compiler — pick one:**

| Option | Setup | Build command |
|--------|-------|---------------|
| **cargo-zigbuild** (easiest) | `cargo install cargo-zigbuild` + `brew install zig` | `cargo zigbuild --target aarch64-unknown-linux-musl --release` |
| **musl-cross toolchain** | Install `aarch64-linux-musl-gcc` (configure in `.cargo/config.toml`) | `cargo build --target aarch64-unknown-linux-musl --release` |
| **Buildroot toolchain** | Run a Buildroot build first, then point `CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER` at its GCC | `cargo build --target aarch64-unknown-linux-musl --release` |

## Quick Start — Build & Test

### 1. Cross-compile the Rust binaries

```bash
# Using cargo-zigbuild:
cargo zigbuild --target aarch64-unknown-linux-musl --release

# Or with a musl-cross toolchain already installed:
cargo build --target aarch64-unknown-linux-musl --release
```

Outputs land in `target/aarch64-unknown-linux-musl/release/`.

> **Note:** `flxc1000-app` depends on `rppal` (Linux-only GPIO). It will only
> compile when targeting Linux. Host-only `cargo check` works for all crates
> except `flxc1000-app`.

### 2. Run without hardware

```bash
# Run a single binary directly — no kernel or Buildroot needed
./scripts/qemu-run.sh boot        # boot variant (flash programming)
./scripts/qemu-run.sh app         # app variant (diagnostics + GPIO sim)

# Full system boot — requires Buildroot kernel + initramfs
./scripts/qemu-run.sh system
```

The script auto-detects your platform:

| Platform | boot/app mode | system mode |
|----------|---------------|-------------|
| **Linux** | `qemu-aarch64` user-mode (`apt install qemu-user-static`) | `qemu-system-aarch64` |
| **macOS** | Docker (`docker run --platform linux/arm64`) | `qemu-system-aarch64` (`brew install qemu`) |

On Apple Silicon, the Docker container runs natively (no emulation overhead).

On macOS, the virtual ECU binds to `127.0.0.2:13400` by default so it doesn't
conflict with a CDA already listening on `127.0.0.1:13400`. Override with
`ECU_IP=127.0.0.3 ./scripts/qemu-run.sh boot` if needed.
GPIO operations are gracefully simulated when `/dev/gpiomem` is absent.

### 3. Test with a DoIP client

Connect any UDS-over-DoIP tester to `localhost:13400` (or the RPi's IP on
hardware). Example with `netcat` for a raw TesterPresent:

```bash
# This is a minimal smoke test — real testing uses a DoIP client library
# The DoIP framing below sends: RoutingActivation + TesterPresent
python3 -c "
import socket, struct
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.connect(('127.0.0.1', 13400))
# Routing Activation Request (source 0x0E00, activation type 0x00)
s.send(bytes.fromhex('02FD000500000007 0E00 00 00000000'.replace(' ','')))
print('RA resp:', s.recv(64).hex())
# DiagnosticMessage: TesterPresent (3E 00) from 0x0E00 to 0x1000
s.send(bytes.fromhex('02FD800100000006 0E00 1000 3E00'.replace(' ','')))
print('Diag ack:', s.recv(64).hex())
print('UDS resp:', s.recv(64).hex())
s.close()
"
```

## Full SD Card Build (Buildroot)

### 1. Clone Buildroot

```bash
git clone https://gitlab.com/buildroot.org/buildroot.git ../buildroot
cd ../buildroot
```

### 2. Configure with the FLXC1000 external tree

```bash
# From the buildroot directory:
make BR2_EXTERNAL=../flxc1000-rpi/buildroot flxc1000_rpi4_defconfig

# Optional: customize further
make menuconfig
```

### 3. Build (first build takes 20–40 min)

```bash
# Make sure Rust binaries are already cross-compiled (step 1 above)
make -j$(nproc)
```

Output in `output/images/`:
- `sdcard.img` — ready to flash
- `Image` — kernel
- `rootfs.cpio.lz4` — initramfs

### 4. Flash to SD card

```bash
# macOS
../flxc1000-rpi/scripts/flash-sdcard.sh /dev/diskN

# Linux
../flxc1000-rpi/scripts/flash-sdcard.sh /dev/sdX
```

The script writes the base image, installs `flxc1000-app` onto partition 2,
and sets `boot_state=app_valid` on partition 3.

### 5. Boot the RPi4

Insert SD card, connect Ethernet, power on. The ECU simulator starts
automatically. Connect to port 13400 on the RPi's IP address.

Serial console is available on the UART pins (115200 8N1).

## Project Structure

```
flxc1000-rpi/
├── .cargo/config.toml          # Cross-compilation linker config
├── Cargo.toml                  # Workspace root
├── flxc1000-init/              # PID 1 init (Linux-only)
├── flxc1000-boot/              # Boot/flash-programming UDS handler
├── flxc1000-app/               # Application UDS handler + GPIO
├── flxc1000-doip/              # Custom DoIP TCP/UDP server library
├── flxc1000-uds/               # (legacy, replaced by ace-server)
├── buildroot/
│   ├── external.desc           # Buildroot external tree descriptor
│   ├── configs/
│   │   └── flxc1000_rpi4_defconfig
│   └── board/flxc1000-rpi/
│       ├── kernel.config       # Kernel config fragment
│       ├── config.txt          # RPi firmware config
│       ├── genimage.cfg        # SD card partition layout
│       ├── post-build.sh       # Copies Rust bins into rootfs
│       ├── post-image.sh       # Runs genimage
│       └── rootfs-overlay/     # Static overlay (mount points)
└── scripts/
    ├── flash-sdcard.sh         # Flash SD card from built image
    └── qemu-run.sh             # Run in QEMU (user or system mode)
```

## UDS Service Details

### Boot Variant (`flxc1000-boot`)

| SID | Service | Sessions | Security |
|-----|---------|----------|----------|
| 0x10 | DiagnosticSessionControl | all | — |
| 0x11 | ECUReset (hard=0x01, soft=0x03) | all | — |
| 0x22 | ReadDataByIdentifier | all | — |
| 0x27 | SecurityAccess (level 0x03) | programming, extended | — |
| 0x34 | RequestDownload | programming | level 0x03 |
| 0x36 | TransferData | programming | level 0x03 |
| 0x37 | RequestTransferExit | programming | level 0x03 |
| 0x3E | TesterPresent | all | — |

**SecurityAccess algorithm:** `key = seed XOR 0xDEADBEEF` (4-byte seed/key).

**DIDs:** 0xF100 (variant ID = `FF0000`), 0xF186 (active session).

### App Variant (`flxc1000-app`)

| SID | Service | Sessions | Notes |
|-----|---------|----------|-------|
| 0x10 | DiagnosticSessionControl | all | |
| 0x11 | ECUReset (hard=0x01) | all | Writes `boot_requested` → boots to bootloader |
| 0x14 | ClearDiagnosticInformation | all | |
| 0x19 | ReadDTCInformation (sub 0x02) | all | |
| 0x22 | ReadDataByIdentifier | all | |
| 0x2E | WriteDataByIdentifier | extended | |
| 0x31 | RoutineControl (0x1001 SelfTest) | extended | LED cascade on 5 GPIOs |
| 0x3E | TesterPresent | all | |

**DIDs:** 0xF100 (variant ID = `000101`), 0xF186 (active session), 0xF190
(VIN, read/write), 0xF200 (FluxCapacitorPower — toggles LED1 during read).

## Hardware — 5-LED Bar Wiring

The self-test routine (RoutineControl 0x1001) drives a bar of 5 LEDs at
increasing brightness via software PWM. Each LED is connected to a GPIO pin
through a current-limiting resistor.

### Bill of Materials

| Qty | Part | Notes |
|-----|------|-------|
| 5 | 3 mm or 5 mm LED (any colour) | All the same colour works well |
| 5 | 330 Ω resistor (¼ W) | 220 Ω–470 Ω all work at 3.3 V |
| 1 | Small perfboard or breadboard | |
| — | Hookup wire / DuPont jumpers | |

### Pin Mapping

All 6 pins sit in a straight line on the left column of the GPIO header
(odd-numbered pins), so the LED board plugs on as a single 6-pin strip.

| LED | GPIO | RPi Header Pin | Duty Cycle |
|-----|------|----------------|------------|
| LED1 (dimmest) | 5 | 29 | 40 % |
| LED2 | 6 | 31 | 55 % |
| LED3 | 13 | 33 | 70 % |
| LED4 | 19 | 35 | 85 % |
| LED5 (brightest) | 26 | 37 | 100 % |
| GND | — | 39 | — |

All LED cathodes share common ground via header pin 39.

### Wiring Diagram

```
  RPi Header — left column, bottom end (pins 29-39)
  ─────────────────────────────────────────────────
  Pin 29 (GPIO 5)  ──┤330 Ω├── LED1 anode ──┐
  Pin 31 (GPIO 6)  ──┤330 Ω├── LED2 anode ──┤
  Pin 33 (GPIO13)  ──┤330 Ω├── LED3 anode ──┤ common cathode
  Pin 35 (GPIO19)  ──┤330 Ω├── LED4 anode ──┤ rail → GND
  Pin 37 (GPIO26)  ──┤330 Ω├── LED5 anode ──┘
  Pin 39 (GND)     ─────────────────────────┘
```

All 6 connections are consecutive odd pins — solder a 6-pin right-angle
header to your perfboard and plug it straight onto the bottom of the
GPIO header.

### Soldering Tips

1. **Orientation** — the flat side / shorter leg of the LED is the cathode (−).
2. Solder the 330 Ω resistors in-line on the anode side — one per LED.
3. Run a single ground bus wire along the perfboard and connect all cathodes.
4. Use a 6-pin single-row header strip — it plugs directly onto RPi header
   pins 29–39 (odd side) with no gaps.
5. Keep leads short to avoid crosstalk on the software-PWM signal.

### Self-Test Behaviour

When RoutineControl SelfTest (0x1001) is triggered:

1. All LEDs turn off.
2. LEDs light up one-by-one (LED1 → LED5), each 200 ms apart, at their
   designated brightness (40 % → 100 %).
3. All 5 LEDs stay on together for 500 ms.
4. All LEDs turn off.

Total duration: ~1.5 s.

## Development Tips

- **Host check (no hardware):** `cargo check -p flxc1000-boot -p flxc1000-doip -p flxc1000-init` compiles on macOS/Linux without issues. `flxc1000-app` requires the Linux target due to `rppal`.
- **Fast iteration:** Use `./scripts/qemu-run.sh boot` — it runs the static aarch64 binary in user-mode emulation with zero boot time.
- **ace-server upgrade:** Update the `rev` in `Cargo.toml` workspace dependencies to pull a newer ace commit.
- **Adding a new UDS service:** Implement the corresponding method on `ServerHandler` in the boot/app `main.rs`. If ace-server doesn't dispatch it (like 0x14/0x19), add a pre-filter in the main loop.
