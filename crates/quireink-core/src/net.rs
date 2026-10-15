//! HTTP requests that plugins ask for and shells make (see
//! `Kernel::take_requests` and `Kernel::http_response`).

use alloc::string::String;
use alloc::vec::Vec;

/// A request for the shell to make, e.g. on a background thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpCall {
    /// Passed back with the response.
    pub id: u32,
    /// `GET`, `PUT`, `POST`, …
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    /// The longest response body wanted; shells cut longer ones.
    pub max_response: usize,
}

/// What came back. `status` 0 means the request failed (no network, no
/// such host, timeout); `body` then holds a short reason.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HttpResponse {
    pub id: u32,
    pub status: u16,
    pub body: Vec<u8>,
    /// The `ETag` header, if any.
    pub etag: Option<String>,
    /// The `Content-Type` header, if any.
    pub content_type: Option<String>,
    /// Where the answer came from after redirects, if elsewhere.
    pub url: Option<String>,
}

/// What a shell does for plugins' servers (see `Kernel::take_server_calls`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerCall {
    /// Listen on `port` in the local network.
    Listen {
        server: u32,
        port: u16,
    },
    Stop {
        server: u32,
    },
    /// Answer request `request` with `body`, or with `file`.
    Respond {
        request: u32,
        status: u16,
        content_type: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        file: Option<ServedFile>,
    },
}

/// A file sent as an answer: the shell reads it in parts with
/// `Kernel::read_served`, so it never needs to fit in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServedFile {
    pub path: String,
    pub size: u64,
}

/// What a shell tells about servers (see `Kernel::server_event`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    /// Listening; reached at `address` (e.g. `192.168.1.20:8080`).
    Started {
        server: u32,
        address: String,
    },
    Failed {
        server: u32,
        reason: String,
    },
    /// A request; answer it with the same `request`. With a `body`, its
    /// parts follow as [`ServerEvent::Body`]; the plugin hears of the
    /// request once the last part came.
    Request {
        server: u32,
        request: u32,
        method: String,
        target: String,
        body: bool,
    },
    /// Part of a request's body, saved as it comes (so it never needs to
    /// fit in memory); `last` with the final part.
    Body {
        request: u32,
        data: Vec<u8>,
        last: bool,
    },
    /// The connection of a request was lost.
    Closed {
        request: u32,
    },
}

/// Longest response body passed on to plugins.
pub const MAX_RESPONSE: usize = 1 << 20;
/// Longest response body saved to a file, and longest file sent.
pub const MAX_FILE: usize = 64 << 20;

/// What a shell does for speech (see `Kernel::take_speech`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechCall {
    /// Speak `text` instead of anything spoken now, and report with
    /// `Kernel::speech_done` and `id` when it is done.
    Say {
        id: u32,
        text: String,
        /// E.g. `de` or `en-GB`; empty for the system's language.
        language: String,
        /// In percent of the normal speed.
        rate: u16,
    },
    /// Stop speaking.
    Stop,
}
