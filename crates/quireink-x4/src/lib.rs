//! The Xteink X4's hardware, for QuireInk's firmware: its display, its
//! microSD card, its buttons and its battery. Written against `embedded-hal`, so all of it
//! is tested on a computer, against simulated hardware; the firmware only
//! connects the ESP32-C3's pins to it.
//!
//! What the X4 has (from the community's documentation of it):
//!
//! | Part | Connection |
//! |---|---|
//! | 4.26" panel, 800 × 480, SSD1677 controller | SPI: clock GPIO 8, data out GPIO 10, select GPIO 21; data/command GPIO 4, reset GPIO 5, busy GPIO 6 |
//! | microSD card | the same SPI (data in GPIO 7), select GPIO 12 |
//! | Back, confirm, left, right | resistor ladder on ADC GPIO 1 |
//! | The side buttons (up, down) | resistor ladder on ADC GPIO 2 |
//! | Power button | GPIO 3, low while pressed |
//! | Battery | half its voltage on ADC GPIO 0 |
//! | USB connected | GPIO 20, high |
//!
//! QuireInk draws the screen upright, 480 × 800, in bands of rows; the
//! display writes them turned into the controller's memory, which keeps
//! the frame, so the firmware holds none (see [`display`]).

#![no_std]

extern crate alloc;

pub mod battery;
pub mod buttons;
pub mod card;
pub mod display;
