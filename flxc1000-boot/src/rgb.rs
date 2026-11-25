use rs_ws281x::{ChannelBuilder, ControllerBuilder, StripType};

use std::{
    sync::{atomic::Ordering, Arc},
    thread,
    time::Duration,
};

use super::SharedState;

const LED_COUNT: i32 = 24;
const UPDATE_DELAY_MS: u64 = 60;

pub fn spawn_update_task(shared: Arc<SharedState>) {
    thread::spawn(move || {
        if let Err(e) = run(shared) {
            tracing::warn!("Boot RGB update animation disabled: {}", e);
        }
    });
}

fn run(shared: Arc<SharedState>) -> Result<(), String> {
    let mut controller = ControllerBuilder::new()
        .freq(800_000)
        .dma(10)
        .channel(
            0,
            ChannelBuilder::new()
                .pin(18)
                .count(24)
                .strip_type(StripType::Ws2812)
                .brightness(32)
                .build(),
        )
        .build()
        .map_err(|e| format!("Failed to initialize boot RGB LEDs: {e:?}"))?;

    let mut position = 0usize;

    loop {
        let update_active = shared.update_active.load(Ordering::Relaxed);

        if update_active {
            // Clear LEDs first
            for led in controller.leds_mut(0) {
                *led = [0, 0, 0, 0];
            }

            // Bright white/cyan moving point
            controller.leds_mut(0)[position] = [220, 255, 255, 0];

            // Slight trail
            if position > 0 {
                controller.leds_mut(0)[position - 1] = [70, 100, 110, 0];
            }

            position = (position + 1) % 24;
        } else {
            for led in controller.leds_mut(0) {
                *led = [0, 0, 0, 0];
            }

            position = 0;
        }

        controller
            .render()
            .map_err(|e| format!("Failed to render update LEDs: {e:?}"))?;

        thread::sleep(Duration::from_millis(60));
    }
}
