use rs_ws281x::{ChannelBuilder, ControllerBuilder, StripType};
// Imports 3 things from the library we use
// ControllerBuilder   - creates the overall LED controller
// ChannelBuilder      - configures one LED output channel, ex: GPIO18
// StripType           - tells the library what kind of LED strip we use, ws2812

use std::{
    sync::{
        atomic::Ordering, // safely read atomic shared variables
        Arc,              // share the same data between different parts of the program
    },
    thread,         // run the RGB logic in the background
    time::Duration, // represents time, like 50 ms
};

use super::SharedState;

const COMMUNICATION_DELAY_MS: u64 = 45;
const ETHERNET_LED_COUNT: i32 = 24;
const SELF_TEST_LED_COUNT: i32 = 8;

const BRANCH_COUNT: usize = 3;
const BRANCH_LENGTH: usize = 8;
const ANIMATION_DELAY_MS: u64 = 90;

// Brightness levels corresponding to the old 8 LEDs
const LEVELS: [u8; 8] = [80, 105, 130, 155, 180, 205, 230, 255];

pub fn spawn_status_task(shared: Arc<SharedState>) {
    thread::spawn(move || {
        if let Err(e) = run(shared) {
            tracing::warn!("RGB status disabled: {}", e);
        }
    });
}
fn run(shared: Arc<SharedState>) -> Result<(), String> {
    let mut controller = ControllerBuilder::new()
        .freq(800_000)
        .dma(10)
        // RGB #1 - main Flux Capacitor LEDs
        // GPIO18 = physical pin 12
        .channel(
            0,
            ChannelBuilder::new()
                .pin(18)
                .count(ETHERNET_LED_COUNT)
                .strip_type(StripType::Ws2812)
                .brightness(32)
                .build(),
        )
        // RGB #2 - old 8 LED bar / self-test
        // GPIO13 = physical pin 33
        .channel(
            1,
            ChannelBuilder::new()
                .pin(13)
                .count(SELF_TEST_LED_COUNT)
                .strip_type(StripType::Ws2812)
                .brightness(32)
                .build(),
        )
        .build()
        .map_err(|e| format!("Failed to initialize RGB LEDs: {e:?}"))?;

    let mut animation_position: usize = 0;

    loop {
        let connected = ethernet_connected();

        let diagnostic_active = shared.diagnostic_activity.load(Ordering::Relaxed);

        if !connected {
            // ------------------------------------
            // Ethernet disconnected
            // ------------------------------------

            let leds = controller.leds_mut(0);

            for led in leds.iter_mut() {
                *led = [0, 0, 255, 0];
            }

            controller
                .render()
                .map_err(|e| format!("Failed to render RGB LEDs: {e:?}"))?;
        } else if diagnostic_active {
            // ------------------------------------
            // Diagnostic communication active
            // ------------------------------------

            render_flux_communication(&mut controller, animation_position)?;

            animation_position += 1;

            if animation_position >= BRANCH_LENGTH {
                animation_position = 0;

                // One communication animation finished,
                // return to idle afterwards
                shared.diagnostic_activity.store(false, Ordering::Relaxed);
            }

            thread::sleep(Duration::from_millis(COMMUNICATION_DELAY_MS));

            continue;
        } else {
            // ------------------------------------
            // Normal system state
            // Flux Capacitor idle animation
            // ------------------------------------

            render_flux_idle(&mut controller, animation_position)?;

            if animation_position == BRANCH_LENGTH - 1 {
                thread::sleep(Duration::from_millis(200));
            }

            animation_position += 1;

            if animation_position >= BRANCH_LENGTH {
                animation_position = 0;
            }
        }

        thread::sleep(Duration::from_millis(ANIMATION_DELAY_MS));
    }
}

fn render_flux_idle(controller: &mut rs_ws281x::Controller, position: usize) -> Result<(), String> {
    let leds = controller.leds_mut(0);

    // Soft background glow
    for led in leds.iter_mut() {
        *led = [5, 15, 30, 0];
    }

    // Pulse on all three branches
    for branch in 0..BRANCH_COUNT {
        let base = branch * BRANCH_LENGTH;

        // Stick 3 is physically reversed
        let current = if branch == 2 {
            base + (BRANCH_LENGTH - 1 - position)
        } else {
            base + position
        };

        leds[current] = [120, 220, 255, 0];

        if position >= 1 {
            let trail_1 = if branch == 2 {
                base + (BRANCH_LENGTH - position)
            } else {
                base + position - 1
            };

            leds[trail_1] = [50, 120, 200, 0];
        }

        if position >= 2 {
            let trail_2 = if branch == 2 {
                base + (BRANCH_LENGTH + 1 - position)
            } else {
                base + position - 2
            };

            leds[trail_2] = [15, 45, 90, 0];
        }
    }

    // When the pulse reaches the center,
    // make the three center LEDs flash brighter
    if position == BRANCH_LENGTH - 1 {
        leds[7] = [180, 255, 255, 0];
        leds[15] = [180, 255, 255, 0];
        leds[16] = [180, 255, 255, 0];
    }

    controller
        .render()
        .map_err(|e| format!("Failed to render Flux Idle animation: {e:?}"))
}

fn render_flux_communication(
    controller: &mut rs_ws281x::Controller,
    position: usize,
) -> Result<(), String> {
    let leds = controller.leds_mut(0);

    // Dark blue background
    for led in leds.iter_mut() {
        *led = [3, 10, 20, 0];
    }

    for branch in 0..BRANCH_COUNT {
        let base = branch * BRANCH_LENGTH;

        // Stick 3 is mounted in the opposite direction
        let current = if branch == 2 {
            base + (BRANCH_LENGTH - 1 - position)
        } else {
            base + position
        };

        // Bright communication pulse
        leds[current] = [180, 255, 255, 0];

        // First trail LED
        if position >= 1 {
            let trail_1 = if branch == 2 {
                base + (BRANCH_LENGTH - position)
            } else {
                base + position - 1
            };

            leds[trail_1] = [80, 200, 255, 0];
        }

        // Second trail LED
        if position >= 2 {
            let trail_2 = if branch == 2 {
                base + (BRANCH_LENGTH + 1 - position)
            } else {
                base + position - 2
            };

            leds[trail_2] = [30, 90, 160, 0];
        }
    }

    // Bright center flash
    if position == BRANCH_LENGTH - 1 {
        leds[7] = [255, 255, 255, 0];
        leds[15] = [255, 255, 255, 0];
        leds[16] = [255, 255, 255, 0];
    }

    controller
        .render()
        .map_err(|e| format!("Failed to render communication animation: {e:?}"))
}

fn ethernet_connected() -> bool {
    let interfaces = match std::fs::read_dir("/sys/class/net") {
        Ok(entries) => entries,
        Err(_) => return false,
    };

    for entry in interfaces.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();

        if name == "lo" {
            continue;
        }

        let carrier_path = entry.path().join("carrier");

        if let Ok(value) = std::fs::read_to_string(carrier_path) {
            if value.trim() == "1" {
                return true;
            }
        }
    }

    false
}
