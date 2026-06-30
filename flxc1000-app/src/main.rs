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

use std::ffi::CString;
use std::fs;
use std::process;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ace_server::config::*;
use ace_server::handler::ServerHandler;
use ace_server::nrc::BuiltinNrc;
use ace_server::security_provider::{SecurityError, SecurityProvider};
use ace_server::server::{UdsServer, MAX_FRAME, MAX_OUTBOX};
use ace_sim::clock::Instant as AceInstant;
use ace_sim::io::NodeAddress;

use flxc1000_doip::server::{get_mac_address, DoipConfig, DoipServer};

const ECU_ADDRESS: u16 = 0x1000;
const VIN: &[u8; 17] = b"FLXC1000TEST00001";

/// App variant identification: DID 0xF100 = 0x000101
const VARIANT_ID: [u8; 3] = [0x00, 0x01, 0x01];

const BOOT_STATE_PATH: &str = "/data/boot_state";

/// GPIO pins for the 5-LED bar (active-high, header pins 29-37 odd).
/// LED1 (dimmest) → LED5 (brightest). GND on pin 39.
const LED_PINS: [u8; 5] = [5, 6, 13, 19, 26];

/// PWM duty cycles for each LED (0.0–1.0) — increasing brightness.
const LED_DUTY: [f64; 5] = [0.40, 0.55, 0.70, 0.85, 1.00];

/// Software-PWM frequency for LED brightness control (Hz).
const LED_PWM_FREQ: f64 = 1000.0;

const RESET_NONE: u8 = 0;
const RESET_HARD: u8 = 1;

/// Routine status codes (routineStatusRecord byte)
const ROUTINE_IDLE: u8 = 0x00;
const ROUTINE_RUNNING: u8 = 0x01;
const ROUTINE_COMPLETED: u8 = 0x02;
const ROUTINE_ABORTED: u8 = 0x03;

/// Hardcoded test DTCs
const DTC_CODE1: [u8; 3] = [0x01, 0xE2, 0x40]; // 0x01E240
const DTC_CODE2: [u8; 3] = [0x03, 0x94, 0x47]; // 0x039447

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Dtc {
    code: [u8; 3],
    status: u8,
}

struct SharedState {
    pending_reset: AtomicU8,
    session_type: AtomicU8,
    routine_status: AtomicU8,
    vin: Mutex<[u8; 17]>,
    dtcs: Mutex<Vec<Dtc>>,
    gpio: Option<rppal::gpio::Gpio>,
    flux_capacitor_power: i32,
}

impl SharedState {
    /// Set a single LED on or off (full brightness, no PWM).
    fn set_led(&self, index: usize, high: bool) {
        let pin_num = LED_PINS[index];
        if let Some(ref gpio) = self.gpio {
            match gpio.get(pin_num) {
                Ok(pin) => {
                    let mut pin = pin.into_output();
                    pin.set_reset_on_drop(false);
                    if high {
                        pin.set_high();
                    } else {
                        pin.set_low();
                    }
                }
                Err(e) => tracing::warn!("GPIO pin {} access failed: {}", pin_num, e),
            }
        } else {
            tracing::debug!(
                "GPIO simulated: LED{} (pin {}) = {}",
                index + 1,
                pin_num,
                if high { "HIGH" } else { "LOW" }
            );
        }
    }

    /// Turn all LEDs off.
    fn all_leds_off(&self) {
        for i in 0..LED_PINS.len() {
            self.set_led(i, false);
        }
    }

    /// Light a single LED at its designated PWM brightness for `duration_ms`,
    /// then turn it off.
    fn pulse_led(&self, index: usize, duration_ms: u64) {
        let pin_num = LED_PINS[index];
        let duty = LED_DUTY[index];
        if let Some(ref gpio) = self.gpio {
            match gpio.get(pin_num) {
                Ok(pin) => {
                    let mut pin = pin.into_output();
                    pin.set_reset_on_drop(false);
                    if let Err(e) = pin.set_pwm_frequency(LED_PWM_FREQ, duty) {
                        tracing::warn!(
                            "PWM on pin {} failed: {} — falling back to digital",
                            pin_num,
                            e
                        );
                        pin.set_high();
                    }
                    std::thread::sleep(Duration::from_millis(duration_ms));
                    let _ = pin.clear_pwm();
                    pin.set_low();
                }
                Err(e) => {
                    tracing::warn!("GPIO pin {} access failed: {}", pin_num, e);
                    std::thread::sleep(Duration::from_millis(duration_ms));
                }
            }
        } else {
            tracing::debug!(
                "GPIO simulated: LED{} (pin {}) PWM duty={:.0}% for {}ms",
                index + 1,
                pin_num,
                duty * 100.0,
                duration_ms,
            );
            std::thread::sleep(Duration::from_millis(duration_ms));
        }
    }

    /// Self-test LED cascade: lights LEDs 1→5 one after another, each at its
    /// designated brightness, holds all on briefly, then turns all off.
    /// Updates `routine_status` atomically so the handler can poll it.
    fn led_cascade_test(&self) {
        self.routine_status.store(ROUTINE_RUNNING, Ordering::SeqCst);
        self.all_leds_off();

        // Keep all OutputPin handles alive so their PWM threads persist.
        let mut active_pins: Vec<rppal::gpio::OutputPin> = Vec::with_capacity(LED_PINS.len());

        // Light each LED in sequence (previous ones stay on)
        for i in 0..LED_PINS.len() {
            // Check for abort between LEDs
            if self.routine_status.load(Ordering::SeqCst) == ROUTINE_ABORTED {
                tracing::info!("SelfTest aborted during cascade");
                drop(active_pins);
                self.all_leds_off();
                return;
            }
            let pin_num = LED_PINS[i];
            let duty = LED_DUTY[i];
            if let Some(ref gpio) = self.gpio {
                if let Ok(pin) = gpio.get(pin_num) {
                    let mut pin = pin.into_output();
                    if let Err(e) = pin.set_pwm_frequency(LED_PWM_FREQ, duty) {
                        tracing::warn!(
                            "PWM on pin {} failed: {} — falling back to digital",
                            pin_num,
                            e
                        );
                        pin.set_high();
                    }
                    active_pins.push(pin);
                }
            } else {
                tracing::debug!(
                    "GPIO simulated: LED{} (pin {}) ON at {:.0}%",
                    i + 1,
                    pin_num,
                    duty * 100.0
                );
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        // Hold all LEDs on for a moment
        std::thread::sleep(Duration::from_millis(500));
        // Drop all handles — stops PWM threads and resets pins
        drop(active_pins);
        self.all_leds_off();
        self.routine_status
            .store(ROUTINE_COMPLETED, Ordering::SeqCst);
        tracing::info!("SelfTest LED cascade completed");
    }

    /// Boot-up blink: flash all 5 LEDs twice to signal the app is ready.
    fn boot_blink(&self) {
        for _ in 0..2 {
            for i in 0..LED_PINS.len() {
                self.set_led(i, true);
            }
            std::thread::sleep(Duration::from_millis(150));
            self.all_leds_off();
            std::thread::sleep(Duration::from_millis(150));
        }
    }
}

// ---------------------------------------------------------------------------
// ServerHandler
// ---------------------------------------------------------------------------

struct AppHandler {
    shared: Arc<SharedState>,
}

impl ServerHandler for AppHandler {
    type Error = BuiltinNrc;

    fn read_did(&self, did: u16, buf: &mut [u8]) -> Result<usize, BuiltinNrc> {
        match did {
            0xF100 => {
                buf[..3].copy_from_slice(&VARIANT_ID);
                Ok(3)
            }
            0xF186 => {
                buf[0] = self.shared.session_type.load(Ordering::Relaxed);
                Ok(1)
            }
            0xF190 => {
                let vin = self.shared.vin.lock().unwrap();
                buf[..17].copy_from_slice(&*vin);
                Ok(17)
            }
            0xF200 => {
                // FluxCapacitorPowerConsumption — toggle LED1 while reading
                self.shared.set_led(0, true);
                let bytes = self.shared.flux_capacitor_power.to_be_bytes();
                buf[..4].copy_from_slice(&bytes);
                self.shared.set_led(0, false);
                Ok(4)
            }
            _ => Err(BuiltinNrc::RequestOutOfRange),
        }
    }

    fn write_did(&mut self, did: u16, data: &[u8]) -> Result<(), BuiltinNrc> {
        match did {
            0xF190 => {
                if data.len() != 17 {
                    return Err(BuiltinNrc::IncorrectMessageLengthOrInvalidFormat);
                }
                let mut vin = self.shared.vin.lock().unwrap();
                vin.copy_from_slice(data);
                tracing::info!("VIN updated to: {}", String::from_utf8_lossy(data));
                Ok(())
            }
            _ => Err(BuiltinNrc::RequestOutOfRange),
        }
    }

    fn ecu_reset(&mut self, reset_type: u8) -> Result<(), BuiltinNrc> {
        match reset_type {
            0x01 => {
                tracing::info!("ECUReset HardReset — switching to bootloader");
                if let Err(e) = fs::write(BOOT_STATE_PATH, "boot_requested\n") {
                    tracing::error!("Failed to write boot_state: {}", e);
                    return Err(BuiltinNrc::ConditionsNotCorrect);
                }
                self.shared
                    .pending_reset
                    .store(RESET_HARD, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(BuiltinNrc::SubFunctionNotSupported),
        }
    }

    fn routine_control(
        &mut self,
        routine_id: u16,
        sub_function: u8,
        _data: &[u8],
        buf: &mut [u8],
    ) -> Result<usize, BuiltinNrc> {
        match (sub_function, routine_id) {
            // Start (0x01) — spawn LED cascade on a background thread
            (0x01, 0x1001) => {
                let status = self.shared.routine_status.load(Ordering::SeqCst);
                if status == ROUTINE_RUNNING {
                    return Err(BuiltinNrc::RequestSequenceError);
                }
                tracing::info!(
                    "RoutineControl SelfTest Start — LED cascade on pins {:?}",
                    LED_PINS
                );
                let shared = Arc::clone(&self.shared);
                std::thread::spawn(move || {
                    shared.led_cascade_test();
                });
                buf[0] = ROUTINE_RUNNING; // routine accepted, running
                Ok(1)
            }
            // Stop (0x02) — abort running routine
            (0x02, 0x1001) => {
                let status = self.shared.routine_status.load(Ordering::SeqCst);
                if status != ROUTINE_RUNNING {
                    return Err(BuiltinNrc::RequestSequenceError);
                }
                tracing::info!("RoutineControl SelfTest Stop — aborting");
                self.shared
                    .routine_status
                    .store(ROUTINE_ABORTED, Ordering::SeqCst);
                buf[0] = ROUTINE_ABORTED;
                Ok(1)
            }
            // RequestResults (0x03) — return current status
            (0x03, 0x1001) => {
                let status = self.shared.routine_status.load(Ordering::SeqCst);
                tracing::debug!(
                    "RoutineControl SelfTest RequestResults — status={:#04x}",
                    status
                );
                buf[0] = status;
                Ok(1)
            }
            _ => Err(BuiltinNrc::RequestOutOfRange),
        }
    }
}

// ---------------------------------------------------------------------------
// SecurityProvider — App variant has no security access
// ---------------------------------------------------------------------------

struct AppSecurity;

impl SecurityProvider for AppSecurity {
    fn generate_seed(&mut self, _level: u8, _buf: &mut [u8]) -> Result<usize, SecurityError> {
        Err(SecurityError::InvalidKey)
    }
    fn validate_key(&self, _level: u8, _seed: &[u8], _key: &[u8]) -> Result<(), SecurityError> {
        Err(SecurityError::InvalidKey)
    }
}

// ---------------------------------------------------------------------------
// DTC services (0x14, 0x19) — handled outside ace-server
// ---------------------------------------------------------------------------

fn handle_clear_dtc(data: &[u8], shared: &SharedState) -> Vec<u8> {
    if data.len() < 4 {
        return vec![0x7F, 0x14, 0x13]; // NRC incorrectMessageLength
    }
    let group = u32::from_be_bytes([0x00, data[1], data[2], data[3]]);
    tracing::info!(
        group = format_args!("0x{:06X}", group),
        "ClearDiagnosticInformation"
    );
    shared.dtcs.lock().unwrap().clear();
    vec![0x54] // positive response SID
}

fn handle_read_dtc(data: &[u8], shared: &SharedState) -> Vec<u8> {
    if data.len() < 2 {
        return vec![0x7F, 0x19, 0x13];
    }
    let report_type = data[1];
    match report_type {
        0x02 => {
            // reportDTCByStatusMask
            if data.len() < 3 {
                return vec![0x7F, 0x19, 0x13];
            }
            let status_mask = data[2];
            let dtcs = shared.dtcs.lock().unwrap();
            let mut resp = vec![0x59, report_type, 0xFF]; // positive response SID + report type + availability mask
            for dtc in dtcs.iter() {
                if dtc.status & status_mask != 0 {
                    resp.extend_from_slice(&dtc.code);
                    resp.push(dtc.status);
                }
            }
            resp
        }
        _ => vec![0x7F, 0x19, 0x12], // NRC subFunctionNotSupported
    }
}

// ---------------------------------------------------------------------------
// Server config
// ---------------------------------------------------------------------------

fn app_server_config() -> ServerConfig {
    ServerConfig::new(ECU_ADDRESS, 0xFFFF)
        // Sessions
        .with_session(SessionConfig::default_session())
        .with_session(SessionConfig::extended_session())
        // Services
        .with_service(ServiceConfig::new(0x10, &[0x01, 0x03])) // DiagnosticSessionControl
        .with_service(ServiceConfig::new(0x11, &[0x01, 0x03])) // ECUReset
        .with_service(ServiceConfig::new(0x22, &[0x01, 0x03])) // ReadDataByIdentifier
        .with_service(ServiceConfig::new(0x2E, &[0x03])) // WriteDataByIdentifier (extended only)
        .with_service(ServiceConfig::new(0x31, &[0x03])) // RoutineControl (extended only)
        .with_service(ServiceConfig::new(0x3E, &[0x01, 0x03])) // TesterPresent
        // DIDs
        .with_did(DidConfig::read_only(0xF100, &[0x01, 0x03])) // variant ID
        .with_did(DidConfig::read_only(0xF186, &[0x01, 0x03])) // session
        .with_did(DidConfig::read_write(0xF190, &[0x01, 0x03], &[0x03])) // VIN (write in extended)
        .with_did(DidConfig::read_only(0xF200, &[0x01, 0x03])) // FluxCapacitorPowerConsumption
}

// ---------------------------------------------------------------------------
// Reset helper
// ---------------------------------------------------------------------------

fn do_hard_reset() -> ! {
    let c_path = CString::new("/init").expect("invalid path");
    let argv = [c_path.clone()];
    let env: Vec<CString> = vec![
        CString::new("PATH=/bin:/sbin").unwrap(),
        CString::new("HOME=/").unwrap(),
    ];
    match nix::unistd::execve(&c_path, &argv, &env) {
        Ok(infallible) => match infallible {},
        Err(e) => {
            tracing::error!("execve /init failed: {}", e);
            process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// ace_sim::clock helper
// ---------------------------------------------------------------------------

fn ace_now(boot: &std::time::Instant) -> AceInstant {
    AceInstant::from_micros(boot.elapsed().as_micros() as u64)
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // To enable persistent logging (pull SD card and read /data/debug.log), set
    // LOG_TO_FILE to true. Disabled by default to reduce flash wear.
    const LOG_TO_FILE: bool = false;

    if LOG_TO_FILE {
        if let Ok(f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/data/debug.log")
        {
            tracing_subscriber::fmt()
                .with_target(false)
                .with_writer(Mutex::new(f))
                .init();
        } else {
            tracing_subscriber::fmt()
                .with_target(false)
                .with_writer(std::io::stderr)
                .init();
        }
    } else {
        tracing_subscriber::fmt()
            .with_target(false)
            .with_writer(std::io::stderr)
            .init();
    }

    tracing::info!(
        "flxc1000-app starting (App variant, address 0x{:04X})",
        ECU_ADDRESS
    );

    // Bring up networking via /etc/network/interfaces (static link-local, carrier-independent)
    configure_network().await;

    let mac = get_mac_address();
    tracing::info!(
        "EID (MAC): {:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        mac[0],
        mac[1],
        mac[2],
        mac[3],
        mac[4],
        mac[5]
    );

    let doip_config = DoipConfig::new(ECU_ADDRESS, VIN).with_eid(mac);
    let doip = DoipServer::new(doip_config);
    let mut uds_rx = doip.run().await.expect("Failed to start DoIP server");

    let gpio = rppal::gpio::Gpio::new().ok();
    if gpio.is_none() {
        tracing::warn!("GPIO not available — GPIO features will be simulated only");
    }

    let shared = Arc::new(SharedState {
        pending_reset: AtomicU8::new(RESET_NONE),
        session_type: AtomicU8::new(0x01),
        routine_status: AtomicU8::new(ROUTINE_IDLE),
        vin: Mutex::new(*VIN),
        dtcs: Mutex::new(vec![
            Dtc {
                code: DTC_CODE1,
                status: 0x2F,
            },
            Dtc {
                code: DTC_CODE2,
                status: 0x24,
            },
        ]),
        gpio,
        flux_capacitor_power: 1210, // 1.21 GW
    });

    let handler = AppHandler {
        shared: Arc::clone(&shared),
    };

    let mut server = UdsServer::new(
        app_server_config(),
        handler,
        AppSecurity,
        NodeAddress(ECU_ADDRESS as u32),
    );

    // Signal boot-up by blinking all LEDs twice
    shared.boot_blink();

    let boot_time = std::time::Instant::now();
    tracing::info!("App ECU ready, waiting for diagnostic requests");

    while let Some(req) = uds_rx.recv().await {
        tracing::debug!(
            src = format_args!("0x{:04X}", req.source_address),
            tgt = format_args!("0x{:04X}", req.target_address),
            data = format_args!("{:02X?}", &req.data),
            "UDS request"
        );

        let now = ace_now(&boot_time);
        shared
            .session_type
            .store(server.session_type(), Ordering::Relaxed);

        let response = if !req.data.is_empty() {
            match req.data[0] {
                // DTC services not handled by ace-server — pre-filter
                0x14 => handle_clear_dtc(&req.data, &shared),
                0x19 => handle_read_dtc(&req.data, &shared),
                _ => {
                    let src = NodeAddress(req.source_address as u32);
                    let _ = server.handle(&src, &req.data, now);
                    let _ = server.tick(now);
                    shared
                        .session_type
                        .store(server.session_type(), Ordering::Relaxed);

                    let mut outbox: heapless::Vec<
                        (NodeAddress, heapless::Vec<u8, MAX_FRAME>),
                        MAX_OUTBOX,
                    > = heapless::Vec::new();
                    server.drain_outbox(&mut outbox);

                    outbox
                        .into_iter()
                        .find(|(dst, _)| dst == &src)
                        .map(|(_, data)| data.to_vec())
                        .unwrap_or_default()
                }
            }
        } else {
            vec![0x7F, 0x00, 0x13]
        };

        tracing::debug!(data = format_args!("{:02X?}", &response), "UDS response");

        if let Err(e) = req.response_tx.send(response).await {
            tracing::error!("Failed to send UDS response: {}", e);
        }

        // Check for pending reset AFTER response is sent
        let reset = shared.pending_reset.swap(RESET_NONE, Ordering::SeqCst);
        if reset == RESET_HARD {
            tokio::time::sleep(Duration::from_millis(50)).await;
            do_hard_reset();
        }
    }
}

async fn configure_network() {
    // Wait for USB-Ethernet to enumerate (RPi3 B+ LAN7515 takes a few seconds)
    let iface = wait_for_eth_interface().await;

    // Equivalent to busybox ifup with /etc/network/interfaces "auto eth0 / static":
    //   ip link set <iface> up
    //   ip addr add <addr>/<mask> dev <iface>
    // The interface comes up immediately, carrier-independent.
    let steps: &[&[&str]] = &[
        &["link", "set", &iface, "up"],
        &["addr", "add", "169.254.16.1/16", "dev", &iface],
    ];

    for args in steps {
        match tokio::process::Command::new("ip")
            .args(*args)
            .output()
            .await
        {
            Ok(output) if !output.status.success() => {
                tracing::warn!(
                    "ip {:?}: {}",
                    args,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
            Err(e) => tracing::error!("ip {:?}: {}", args, e),
            _ => {}
        }
    }

    tracing::info!("Network configured: 169.254.16.1/16 on {}", iface);
}

/// Wait up to 10 s for a non-loopback network interface in /sys/class/net.
async fn wait_for_eth_interface() -> String {
    for attempt in 0..20 {
        if let Ok(entries) = std::fs::read_dir("/sys/class/net") {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name != "lo" {
                    tracing::info!("Found network interface: {} (attempt {})", name, attempt);
                    return name;
                }
            }
        }
        if attempt == 0 {
            tracing::info!("Waiting for network interface to appear...");
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    tracing::warn!("No network interface found after 10s, defaulting to eth0");
    "eth0".to_string()
}
