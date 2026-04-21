use std::sync::Mutex;

use mirajazz::{error::MirajazzError, types::DeviceInput};

use crate::mappings::KEY_COUNT;

static BUTTON_STATE: Mutex<[u8; KEY_COUNT]> = Mutex::new([0u8; KEY_COUNT]);

// Bottom button input codes (non-LCD buttons)
const BTN_LEFT: u8 = 0x25;
const BTN_MIDDLE: u8 = 0x30;
const BTN_RIGHT: u8 = 0x31;

pub fn process_input(input: u8, state: u8) -> Result<DeviceInput, MirajazzError> {
    log::debug!("Processing input: key={}, state={}", input, state);

    match input {
        0..=15 => read_button_press(input, state),
        BTN_LEFT | BTN_MIDDLE | BTN_RIGHT => read_button_press(input, state),
        _ => Err(MirajazzError::BadData),
    }
}

fn read_button_states(states: &[u8]) -> Vec<bool> {
    let mut bools = vec![];

    for i in 0..KEY_COUNT {
        bools.push(states[i + 1] != 0);
    }

    bools
}

/// Flips row order: row 0 ↔ row 2, row 1 stays.
/// Device is vertically flipped compared to OpenDeck.
fn flip_row(key: u8) -> u8 {
    let row = key / 5;
    let col = key % 5;
    (2 - row) * 5 + col
}

/// Converts opendeck key index to device key index (for sending images)
pub fn opendeck_to_device(key: u8) -> u8 {
    flip_row(key)
}

fn read_button_press(input: u8, state: u8) -> Result<DeviceInput, MirajazzError> {
    if input == 0 {
        let states = BUTTON_STATE.lock().unwrap();
        let mut button_states = vec![0x01u8];
        button_states.extend_from_slice(&*states);
        return Ok(DeviceInput::ButtonStateChange(read_button_states(
            &button_states,
        )));
    }

    // Map input to OpenDeck button index (0-based)
    let pressed_index: usize = match input {
        // LCD buttons (1-15 from device, map to 0-14)
        1..=15 => (input - 1) as usize,
        // Bottom buttons (non-LCD, map to 15-17)
        BTN_LEFT => 15,
        BTN_MIDDLE => 16,
        BTN_RIGHT => 17,
        _ => return Err(MirajazzError::BadData),
    };

    let mut states = BUTTON_STATE.lock().unwrap();
    states[pressed_index] = state;

    let mut button_states = vec![0x01u8];
    button_states.extend_from_slice(&*states);

    Ok(DeviceInput::ButtonStateChange(read_button_states(
        &button_states,
    )))
}
