use device::{handle_error, handle_set_image, set_led_colors};
use mirajazz::device::Device;
use openaction::*;
use std::{collections::HashMap, process::exit, sync::LazyLock};
use tokio::sync::{Mutex, RwLock};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use watcher::watcher_task;

#[cfg(not(target_os = "windows"))]
use tokio::signal::unix::{SignalKind, signal};

mod device;
mod inputs;
mod mappings;
mod watcher;

pub static DEVICES: LazyLock<RwLock<HashMap<String, Device>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
pub static TOKENS: LazyLock<RwLock<HashMap<String, CancellationToken>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
pub static TRACKER: LazyLock<Mutex<TaskTracker>> = LazyLock::new(|| Mutex::new(TaskTracker::new()));
type LedColors = [(u8, u8, u8); 24];

pub static LED_COLORS: LazyLock<RwLock<Option<LedColors>>> =
    LazyLock::new(|| RwLock::new(None));

struct GlobalEventHandler {}
impl openaction::GlobalEventHandler for GlobalEventHandler {
    async fn plugin_ready(
        &self,
        _outbound: &mut openaction::OutboundEventManager,
    ) -> EventHandlerResult {
        let tracker = TRACKER.lock().await.clone();

        let token = CancellationToken::new();
        tracker.spawn(watcher_task(token.clone()));

        TOKENS
            .write()
            .await
            .insert("_watcher_task".to_string(), token);

        log::info!("Plugin initialized");

        Ok(())
    }

    async fn set_image(
        &self,
        event: SetImageEvent,
        _outbound: &mut OutboundEventManager,
    ) -> EventHandlerResult {
        log::debug!("Asked to set image: {:#?}", event);

        // Skip knobs images
        if event.controller == Some("Encoder".to_string()) {
            log::debug!("Looks like a knob, no need to set image");
            return Ok(());
        }

        let id = event.device.clone();

        let result = {
            let devices = DEVICES.read().await;
            match devices.get(&id) {
                Some(device) => Some(handle_set_image(device, event).await),
                None => None,
            }
        }; // Read guard dropped here

        match result {
            Some(Err(e)) => {
                let _ = handle_error(&id, e).await;
            }
            None => log::error!("Received event for unknown device: {}", id),
            _ => {}
        }

        Ok(())
    }

    async fn set_brightness(
        &self,
        event: SetBrightnessEvent,
        _outbound: &mut OutboundEventManager,
    ) -> EventHandlerResult {
        log::debug!("Asked to set brightness: {:#?}", event);

        let id = event.device.clone();

        let result = {
            let devices = DEVICES.read().await;
            match devices.get(&id) {
                Some(device) => Some(device.set_brightness(event.brightness).await),
                None => None,
            }
        }; // Read guard dropped here

        match result {
            Some(Err(e)) => {
                let _ = handle_error(&id, e).await;
            }
            None => log::error!("Received event for unknown device: {}", id),
            _ => {}
        }

        Ok(())
    }

    async fn did_receive_global_settings(
        &self,
        event: DidReceiveGlobalSettingsEvent,
        _outbound: &mut OutboundEventManager,
    ) -> EventHandlerResult {
        log::info!("Received global settings: {:#?}", event);

        if let Some(settings) = event.payload.settings.as_object() {
            if let Some(colors) = parse_led_colors_from_settings(settings) {
                log::info!("Setting default LED colors from global settings");
                *LED_COLORS.write().await = Some(colors);

                // Apply to all connected devices
                let device_ids: Vec<String> = DEVICES.read().await.keys().cloned().collect();
                log::info!("Applying to {} connected devices", device_ids.len());

                for id in device_ids {
                    let result = {
                        let devices = DEVICES.read().await;
                        match devices.get(&id) {
                            Some(device) => Some(set_led_colors(device, &colors).await),
                            None => None,
                        }
                    }; // Read guard dropped here

                    if let Some(Err(e)) = result {
                        let _ = handle_error(&id, e).await;
                    }
                }
            }
        }

        Ok(())
    }
}

struct ActionEventHandler {}
impl openaction::ActionEventHandler for ActionEventHandler {
    async fn key_down(
        &self,
        event: KeyEvent,
        outbound: &mut OutboundEventManager,
    ) -> EventHandlerResult {
        // Handle Set LED Color action
        if event.action == "com.github.ibanks42.opendeck-m18.set-led-color" {
            log::debug!("Set LED Color action triggered");

            if let Some(settings) = event.payload.settings.as_object() {
                if let Some(colors) = parse_led_colors_from_settings(settings) {
                    log::info!("Setting LED colors");

                    let device_id = event.device.clone();

                    let result = {
                        let devices = DEVICES.read().await;
                        match devices.get(&device_id) {
                            Some(device) => Some(set_led_colors(device, &colors).await),
                            None => None,
                        }
                    }; // Read guard dropped here

                    if let Some(Err(e)) = result {
                        let _ = handle_error(&device_id, e).await;
                    }

                    // Check if we should save to global settings
                    let save_global = settings
                        .get("saveGlobal")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);

                    if save_global {
                        // Update in-memory cache
                        *LED_COLORS.write().await = Some(colors);

                        // Persist via OpenDeck's global settings
                        let global_settings = serde_json::json!({
                            "ledColors": colors.iter()
                                .map(|(r, g, b)| format!("#{:02x}{:02x}{:02x}", r, g, b))
                                .collect::<Vec<_>>()
                        });
                        if let Err(e) = outbound.set_global_settings(global_settings).await {
                            log::error!("Failed to save global settings: {}", e);
                        }
                    }
                }
            }
        }

        // Handle Load LED Color action
        if event.action == "com.github.ibanks42.opendeck-m18.load-led-color" {
            log::debug!("Load LED Color action triggered");

            if let Some(colors) = LED_COLORS.read().await.as_ref() {
                log::info!("Loading saved LED colors");
                let device_id = event.device.clone();

                let result = {
                    let devices = DEVICES.read().await;
                    match devices.get(&device_id) {
                        Some(device) => Some(set_led_colors(device, colors).await),
                        None => None,
                    }
                };

                if let Some(Err(e)) = result {
                    let _ = handle_error(&device_id, e).await;
                }
            } else {
                log::warn!("No saved LED colors found");
            }
        }

        Ok(())
    }
}

/// Parses a hex color string (#RRGGBB) to RGB values
fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Parses LED colors from a settings object (expected format: {"ledColors": ["#RRGGBB", ...]})
fn parse_led_colors_from_settings(settings: &serde_json::Map<String, serde_json::Value>) -> Option<LedColors> {
    let colors_arr = settings.get("ledColors")?.as_array()?;

    if colors_arr.len() != 24 {
        log::warn!("LED colors array has wrong length: {}", colors_arr.len());
        return None;
    }

    let mut colors = [(0u8, 0u8, 0u8); 24];
    for (i, color_val) in colors_arr.iter().enumerate() {
        let color_str = color_val.as_str()?;
        colors[i] = parse_hex_color(color_str).or_else(|| {
            log::warn!("Invalid hex color at index {}: {:?}", i, color_str);
            None
        })?;
    }

    Some(colors)
}

async fn shutdown() {
    let tokens = TOKENS.write().await;

    for (_, token) in tokens.iter() {
        token.cancel();
    }
}

async fn connect() {
    if let Err(error) = init_plugin(GlobalEventHandler {}, ActionEventHandler {}).await {
        log::error!("Failed to initialize plugin: {}", error);

        exit(1);
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
async fn sigterm() -> Result<(), Box<dyn std::error::Error>> {
    let mut sig = signal(SignalKind::terminate())?;

    sig.recv().await;

    Ok(())
}

#[cfg(target_os = "windows")]
async fn sigterm() -> Result<(), Box<dyn std::error::Error>> {
    // Future that would never resolve, so select only acts on OpenDeck connection loss
    // TODO: Proper windows termination handling
    std::future::pending::<()>().await;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    simplelog::TermLogger::init(
        simplelog::LevelFilter::Info,
        simplelog::Config::default(),
        simplelog::TerminalMode::Stdout,
        simplelog::ColorChoice::Never,
    )
    .unwrap();

    tokio::select! {
        _ = connect() => {},
        _ = sigterm() => {},
    }

    log::info!("Shutting down");

    shutdown().await;

    let tracker = TRACKER.lock().await.clone();

    log::info!("Waiting for tasks to finish");

    tracker.close();
    tracker.wait().await;

    log::info!("Tasks are finished, exiting now");

    Ok(())
}
