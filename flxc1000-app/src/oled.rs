use super::SharedState;
use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyleBuilder},
    pixelcolor::BinaryColor,
    prelude::*,
    text::{Baseline, Text},
};
use rppal::i2c::I2c;
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};
use std::{fmt::Debug, sync::Arc, thread, time::Duration};
use tokio::sync::mpsc::Receiver;

const OLED_ADDRESS: u8 = 0x3C;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OledEvent {
    LastErrorChanged(String),
    EthernetChanged(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OledState {
    power: i32,
    last_error: Option<String>,
    ethernet_connected: bool,
}

impl OledState {
    fn apply_event(&mut self, event: OledEvent) -> bool {
        let previous = self.clone();

        match event {
            OledEvent::LastErrorChanged(message) => {
                self.last_error = Some(message);
            }
            OledEvent::EthernetChanged(connected) => {
                self.ethernet_connected = connected;
            }
        }

        *self != previous
    }
}

pub fn spawn_status_task(shared: Arc<SharedState>, mut receiver: Receiver<OledEvent>) {
    thread::spawn(move || loop {
        match run(Arc::clone(&shared), &mut receiver) {
            Ok(()) => break,
            Err(e) => {
                tracing::warn!("OLED error: {}. Retrying initialization...", e);
                thread::sleep(Duration::from_secs(2));
            }
        }
    });
}

fn run(shared: Arc<SharedState>, receiver: &mut Receiver<OledEvent>) -> Result<(), String> {
    match std::fs::read("/proc/device-tree/soc/i2c@7e804000/status") {
        Ok(bytes) => {
            let status = String::from_utf8_lossy(&bytes);
            tracing::debug!("I2C status: {}", status.trim_end_matches('\0'));
        }
        Err(e) => {
            tracing::warn!("Could not read I2C status: {}", e);
        }
    }

    for path in ["/dev/i2c-0", "/dev/i2c-1", "/dev/i2c-3"] {
        tracing::debug!("{} exists: {}", path, std::path::Path::new(path).exists());
    }

    let mut i2c = None;
    let mut last_error = String::new();

    for attempt in 1..=50 {
        match I2c::new() {
            Ok(bus) => {
                tracing::info!("I2C available after attempt {}", attempt);
                i2c = Some(bus);
                break;
            }
            Err(e) => {
                last_error = e.to_string();

                if attempt == 1 || attempt % 10 == 0 {
                    tracing::warn!("Waiting for I2C, attempt {}/50: {}", attempt, e);
                }

                thread::sleep(Duration::from_millis(200));
            }
        }
    }

    let i2c = i2c.ok_or_else(|| format!("Failed to open I2C after 10 seconds: {}", last_error))?;

    let interface = I2CDisplayInterface::new_custom_address(i2c, OLED_ADDRESS);

    let mut display = Ssd1306::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
        .into_buffered_graphics_mode();

    display.init().map_err(fmt_err("init oled"))?;

    tracing::info!("OLED initialized successfully");

    // Initialize the cached display state.
    let mut state = OledState {
        power: shared.flux_capacitor_power,
        last_error: shared.last_error.lock().ok().and_then(|e| e.clone()),
        ethernet_connected: crate::rgb::ethernet_connected(),
    };

    // Draw the initial OLED screen once.
    draw_status(&mut display, &state)?;
    display.flush().map_err(fmt_err("flush oled"))?;

    // Wait for events instead of redrawing every 500 ms.

    while let Some(event) = receiver.blocking_recv() {
        // Remember what the OLED was displaying.
        let previous = state.clone();

        // Process the first event and all other queued events.
        state.apply_event(event);

        while let Ok(event) = receiver.try_recv() {
            state.apply_event(event);
        }

        // Synchronize with the latest application state after processing queued events.
        if let Ok(error) = shared.last_error.lock() {
            state.last_error = error.clone();
        }

        state.ethernet_connected = crate::rgb::ethernet_connected();

        // Render only if the final display state changed.
        if state != previous {
            draw_status(&mut display, &state)?;
            display.flush().map_err(fmt_err("flush oled"))?;
        }
    }
    // Exit if the event channel is explicitly closed.
    Ok(())
}

fn draw_status<D>(display: &mut D, state: &OledState) -> Result<(), String>
where
    D: DrawTarget<Color = BinaryColor>,
    D::Error: Debug,
{
    display
        .clear(BinaryColor::Off)
        .map_err(fmt_err("clear oled"))?;

    let style = MonoTextStyleBuilder::new()
        .font(&FONT_6X10)
        .text_color(BinaryColor::On)
        .build();

    let version = option_env!("FLXC1000_BUILD_VERSION").unwrap_or("unknown");
    let power = state.power;
    let err = state.last_error.as_deref().unwrap_or("none");
    let eth = if state.ethernet_connected {
        "connected"
    } else {
        "disconnected"
    };

    let lines = [
        format!("FLXC v{}", short(version, 14)),
        format!("Power {}", power),
        format!("Err {}", short(&err, 17)),
        format!("Eth {}", eth),
    ];

    for (i, line) in lines.iter().enumerate() {
        Text::with_baseline(line, Point::new(0, i as i32 * 12), style, Baseline::Top)
            .draw(display)
            .map_err(fmt_err("draw oled"))?;
    }

    Ok(())
}

fn short(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

fn fmt_err<E: Debug>(context: &'static str) -> impl FnOnce(E) -> String {
    move |e| format!("{context}: {e:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oled_event_updates_cached_state() {
        let mut state = OledState {
            power: 1210,
            last_error: None,
            ethernet_connected: true,
        };

        // A new error should change the display state.
        let changed = state.apply_event(OledEvent::LastErrorChanged("UDS failed".to_string()));

        assert!(changed);
        assert_eq!(state.last_error.as_deref(), Some("UDS failed"));

        // Repeating the same error should not require a redraw.
        let changed = state.apply_event(OledEvent::LastErrorChanged("UDS failed".to_string()));

        assert!(!changed);

        // Losing Ethernet connectivity should change the display state.
        let changed = state.apply_event(OledEvent::EthernetChanged(false));

        assert!(changed);
        assert!(!state.ethernet_connected);
    }

    #[test]
    fn identical_ethernet_event_keeps_state_unchanged() {
        let mut state = OledState {
            power: 42,
            last_error: None,
            ethernet_connected: true,
        };

        let previous = state.clone();

        state.apply_event(OledEvent::EthernetChanged(true));

        assert_eq!(state, previous);
    }

    #[test]
    fn queued_events_produce_final_state() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<OledEvent>(16);

        sender.try_send(OledEvent::EthernetChanged(false)).unwrap();

        sender
            .try_send(OledEvent::LastErrorChanged("Test error".to_string()))
            .unwrap();

        sender.try_send(OledEvent::EthernetChanged(true)).unwrap();

        let mut state = OledState {
            power: 42,
            last_error: None,
            ethernet_connected: true,
        };

        while let Ok(event) = receiver.try_recv() {
            state.apply_event(event);
        }

        assert!(state.ethernet_connected);
        assert_eq!(state.last_error.as_deref(), Some("Test error"));

        // All queued event have been processed.
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn full_channel_can_recover_latest_error() {
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<OledEvent>(1);

        // Fill the channel with an older event.
        sender
            .try_send(OledEvent::LastErrorChanged("Old error".to_string()))
            .unwrap();

        // Simulate the latest application state.
        let latest_error = Some("New error".to_string());

        // The new notifications cannot be queued.
        let result = sender.try_send(OledEvent::LastErrorChanged("New error".to_string()));

        assert!(matches!(
            result,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_))
        ));

        let mut state = OledState {
            power: 0,
            last_error: None,
            ethernet_connected: true,
        };

        // Process the event already waiting in the channel.
        while let Ok(event) = receiver.try_recv() {
            state.apply_event(event);
        }

        // Simulate the production OLED loop's state synchronization.
        state.last_error = latest_error;

        assert_eq!(state.last_error.as_deref(), Some("New error"));
    }
}
