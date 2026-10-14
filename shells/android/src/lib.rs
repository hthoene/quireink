//! QuireInk for Android e-readers (Boox and others).
//!
//! A native activity in Rust, without Java code: it runs the kernel with the
//! standard plugins, turns touch and key events into QuireInk input and
//! copies rendered frames into the activity's window.
//!
//! Books live in the app's folder on the shared storage
//! (`Android/data/io.github.hthoene.quireink/files/books`); the bundled
//! sample books are copied there on first start.
//!
//! E-ink refresh modes are left to the device for now; vendor SDKs (e.g.
//! Onyx's for Boox) will map QuireInk's refresh requests later.

pub mod keys;
pub mod pixels;

#[cfg(target_os = "android")]
mod app;
