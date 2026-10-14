//! QuireInk for Android e-readers (Boox and others).
//!
//! A native activity in Rust, without Java code: it runs the kernel with the
//! standard plugins, turns touch and key events into QuireInk input and
//! copies rendered frames into the activity's window.
//!
//! Books live in the shared `Books` folder once the user allows access
//! (the home screen offers it; see `access`). Until then, the library is the
//! app's own folder (`Android/data/io.github.hthoene.quireink/files/books`)
//! with the bundled sample books. Settings and caches always stay in the
//! app's folder.
//!
//! E-ink refresh modes are left to the device for now; vendor SDKs (e.g.
//! Onyx's for Boox) will map QuireInk's refresh requests later.

pub mod keys;
pub mod pixels;

#[cfg(target_os = "android")]
mod access;
#[cfg(target_os = "android")]
mod app;
#[cfg(target_os = "android")]
mod plugin;
