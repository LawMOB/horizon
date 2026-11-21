//! Original touch artwork: grey pixel plates, contrasting held icons and a
//! square joystick. Built once with the GUI atlas, using Horizon's own icons.
use horizon_client_input::touch::{TouchButton, icons};

const SIDE: u32 = 22;
pub const STICK: &str = "horizon:gui/sprites/touch/stick";
pub const STICK_KNOB: &str = "horizon:gui/sprites/touch/stick_knob";

/// The atlas sprite for a built-in button, including its held state.
pub fn button_sprite(button: TouchButton, pressed: bool) -> &'static str {
    macro_rules! name {
        ($name:literal) => {
            if pressed {
                concat!("horizon:gui/sprites/touch/", $name, "_pressed")
            } else {
                concat!("horizon:gui/sprites/touch/", $name)
            }
        };
    }
    match button {
        TouchButton::Jump => name!("jump"),
        TouchButton::Sneak => name!("sneak"),
        TouchButton::Attack => name!("attack"),
        TouchButton::Use => name!("use"),
        TouchButton::Drop => name!("drop"),
        TouchButton::Inventory => name!("inventory"),
        TouchButton::Chat => name!("chat"),
        TouchButton::Perspective => name!("camera"),
        TouchButton::Pause => name!("pause"),
        TouchButton::Debug => name!("debug"),
        TouchButton::Sprint => name!("sprint"),
    }
}

/// A blank plate for buttons bound to a custom key or key combination.
pub fn plate_sprite(pressed: bool) -> &'static str {
    if pressed {
        "horizon:gui/sprites/touch/custom_blank_pressed"
    } else {
        "horizon:gui/sprites/touch/custom_blank"
    }
}

/// Atlas entries shared by gameplay and the touch layout editor.
pub(crate) fn images() -> Vec<(String, u32, u32, Vec<u8>)> {
    let mut images = Vec::new();
    for pressed in [false, true] {
        for button in TouchButton::PLACED.into_iter().chain([TouchButton::Sprint]) {
            let mut pixels = plate(SIDE, pressed);
            let ink = if pressed { 235 } else { 24 };
            let offset = (SIDE as usize - icons::SIZE) / 2;
            for (y, row) in icons::icon(button).iter().enumerate() {
                for (x, pixel) in row.bytes().enumerate() {
                    if pixel == b'#' {
                        let start = ((y + offset) * SIDE as usize + x + offset) * 4;
                        pixels[start..start + 4].copy_from_slice(&[ink, ink, ink, 255]);
                    }
                }
            }
            images.push((button_sprite(button, pressed).into(), SIDE, SIDE, pixels));
        }
        images.push((plate_sprite(pressed).into(), SIDE, SIDE, plate(SIDE, pressed)));
    }
    images.push((STICK.into(), 64, 64, stick()));
    images.push((STICK_KNOB.into(), 32, 32, plate(32, false)));
    images
}

/// A stepped corner, dark outer rim and a narrow inset bevel. Each sprite
/// includes the plate and glyph so a control needs only one textured quad.
fn plate(side: u32, pressed: bool) -> Vec<u8> {
    let mut pixels = vec![0; (side * side * 4) as usize];
    for y in 0..side {
        for x in 0..side {
            let dx = x.min(side - 1 - x);
            let dy = y.min(side - 1 - y);
            if dx + dy < 2 {
                continue;
            }
            let value = if dx.min(dy) == 0 {
                if pressed { 235 } else { 24 }
            } else if dx.min(dy) == 1 {
                if pressed {
                    92
                } else if x < 2 || y < 2 {
                    176
                } else {
                    68
                }
            } else {
                128
            };
            let start = ((y * side + x) * 4) as usize;
            pixels[start..start + 4].copy_from_slice(&[value, value, value, 255]);
        }
    }
    pixels
}

fn stick() -> Vec<u8> {
    const SIDE: u32 = 64;
    let mut pixels = vec![0; (SIDE * SIDE * 4) as usize];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let dx = x.min(SIDE - 1 - x);
            let dy = y.min(SIDE - 1 - y);
            if dx + dy < 3 {
                continue;
            }
            let value = if dx.min(dy) == 0 {
                112
            } else if dx >= 12 && dy >= 12 && dx + dy >= 28 {
                208
            } else {
                176
            };
            let start = ((y * SIDE + x) * 4) as usize;
            pixels[start..start + 4].copy_from_slice(&[value, value, value, 255]);
        }
    }
    pixels
}
