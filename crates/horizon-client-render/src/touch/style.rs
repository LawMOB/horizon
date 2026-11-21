//! Embedded touch PNGs, shared by gameplay and the layout editor.
//! Artwork terms and provenance are in `bedrock/NOTICE.txt` and `bedrock/sources.json`.
use horizon_client_input::touch::TouchButton;

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

macro_rules! sprites {
    ($($name:literal),* $(,)?) => {
        &[$((
            concat!("horizon:gui/sprites/touch/", $name),
            include_bytes!(concat!("bedrock/", $name, ".png")) as &[u8],
        )),*]
    };
}

/// Only the artwork used by the touch HUD; decoded when the GUI atlas is built.
pub(crate) const SPRITES: &[(&str, &[u8])] = sprites![
    "attack",
    "attack_pressed",
    "use",
    "use_pressed",
    "jump",
    "jump_pressed",
    "sneak",
    "sneak_pressed",
    "sprint",
    "sprint_pressed",
    "pause",
    "pause_pressed",
    "inventory",
    "inventory_pressed",
    "chat",
    "chat_pressed",
    "camera",
    "camera_pressed",
    "drop",
    "drop_pressed",
    "debug",
    "debug_pressed",
    "custom_blank",
    "custom_blank_pressed",
    "stick",
    "stick_knob",
];
