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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub id: u32,
    pub status: u16,
    pub body: Vec<u8>,
    /// The `ETag` header, if any.
    pub etag: Option<String>,
}

/// Longest response body passed on to plugins.
pub const MAX_RESPONSE: usize = 1 << 20;
/// Longest response body saved to a file, and longest file sent.
pub const MAX_FILE: usize = 64 << 20;
