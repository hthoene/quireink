//! Android key codes to QuireInk button names.
//!
//! E-readers report their page buttons as page or volume keys; keyboards
//! and Bluetooth page turners use arrows, Enter and Space as well.

use quireink_platform::buttons;

// From `android.view.KeyEvent`.
const KEYCODE_BACK: u32 = 4;
const KEYCODE_DPAD_UP: u32 = 19;
const KEYCODE_DPAD_DOWN: u32 = 20;
const KEYCODE_DPAD_LEFT: u32 = 21;
const KEYCODE_DPAD_RIGHT: u32 = 22;
const KEYCODE_DPAD_CENTER: u32 = 23;
const KEYCODE_VOLUME_UP: u32 = 24;
const KEYCODE_VOLUME_DOWN: u32 = 25;
const KEYCODE_SPACE: u32 = 62;
const KEYCODE_ENTER: u32 = 66;
const KEYCODE_MENU: u32 = 82;
const KEYCODE_PAGE_UP: u32 = 92;
const KEYCODE_PAGE_DOWN: u32 = 93;
const KEYCODE_ESCAPE: u32 = 111;

/// The button for an Android key code; `None` leaves the key to Android
/// (e.g. the home key).
pub fn button(key_code: u32) -> Option<&'static str> {
    Some(match key_code {
        KEYCODE_PAGE_DOWN | KEYCODE_VOLUME_DOWN | KEYCODE_SPACE => buttons::PAGE_FORWARD,
        KEYCODE_PAGE_UP | KEYCODE_VOLUME_UP => buttons::PAGE_BACK,
        KEYCODE_DPAD_UP => buttons::UP,
        KEYCODE_DPAD_DOWN => buttons::DOWN,
        KEYCODE_DPAD_LEFT => buttons::LEFT,
        KEYCODE_DPAD_RIGHT => buttons::RIGHT,
        KEYCODE_DPAD_CENTER | KEYCODE_ENTER => buttons::CONFIRM,
        KEYCODE_BACK | KEYCODE_ESCAPE => buttons::BACK,
        KEYCODE_MENU => buttons::MENU,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_keys_turn_pages() {
        assert_eq!(button(KEYCODE_PAGE_DOWN), Some(buttons::PAGE_FORWARD));
        assert_eq!(button(KEYCODE_VOLUME_UP), Some(buttons::PAGE_BACK));
        assert_eq!(button(KEYCODE_BACK), Some(buttons::BACK));
        // The home key (3) stays with Android.
        assert_eq!(button(3), None);
    }
}
