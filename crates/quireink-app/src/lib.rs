//! The standard QuireInk app: the first-party plugins every shell installs.
//!
//! The kernel itself has no product features; a shell decides which plugins
//! it ships. This crate keeps that list in one place, so the emulator, the
//! Android app and the firmware ship the same app. Shells can still add,
//! leave out or replace any plugin.

#![no_std]

use quireink_core::KernelBuilder;

/// How the default fonts are shipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fonts {
    /// Scalable fonts rendered at runtime (needs the `outline` feature).
    #[cfg(feature = "outline")]
    Outline,
    /// Bitmap font packs read in place, for small devices.
    #[cfg(any(feature = "packs", not(feature = "outline")))]
    Packs,
}

/// Adds the first-party plugins to `builder`.
pub fn install(builder: KernelBuilder, fonts: Fonts) -> KernelBuilder {
    let builder = match fonts {
        #[cfg(feature = "outline")]
        Fonts::Outline => builder.plugin(quireink_plugin_fonts_default::DefaultFonts),
        #[cfg(any(feature = "packs", not(feature = "outline")))]
        Fonts::Packs => builder.plugin(quireink_plugin_fonts_default::DefaultFontPacks),
    };
    builder
        .plugin(quireink_plugin_format_txt::TxtFormat)
        .plugin(quireink_plugin_format_epub::EpubFormat)
        .plugin(quireink_plugin_format_xtc::XtcFormat)
        .plugin(quireink_plugin_hyphenation::Hyphenation)
        .plugin(quireink_plugin_dictionary::DictionaryPlugin)
        .plugin(quireink_plugin_reader::Reader)
        .plugin(quireink_plugin_files::Files)
        .plugin(quireink_plugin_home::Home)
        .plugin(quireink_plugin_menu::Menu)
        .plugin(quireink_plugin_menu_editor::MenuEditor)
        .plugin(quireink_plugin_kosync::Kosync)
        .plugin(quireink_plugin_calibre::Calibre)
        .plugin(quireink_plugin_stats::Stats)
        .plugin(quireink_plugin_file_manager::FileManager)
        .plugin(quireink_plugin_transfer::Transfer)
        .plugin(quireink_plugin_opds::Opds)
        .plugin(quireink_plugin_rules::Rules)
        .plugin(quireink_plugin_profiles::Profiles)
        .plugin(quireink_plugin_keys::Keys)
        .plugin(quireink_plugin_wikipedia::Wikipedia)
        .plugin(quireink_plugin_aids::Aids)
        .plugin(quireink_plugin_vocabulary::Vocabulary)
        .plugin(quireink_plugin_sleep::Sleep)
        .plugin(quireink_plugin_translate::Translate)
        .plugin(quireink_plugin_news::News)
        .plugin_off(quireink_plugin_browser::Browser)
        .plugin_off(quireink_plugin_readwise::Readwise)
        .plugin_off(quireink_plugin_wallabag::Wallabag)
        .plugin(quireink_plugin_keyboard::Keyboard)
        .plugin(quireink_plugin_settings::SettingsPlugin)
        .plugin(quireink_plugin_plugin_manager::PluginManager)
        .plugin(quireink_plugin_diagnostics::Diagnostics)
        .plugin(quireink_plugin_about::About)
}
