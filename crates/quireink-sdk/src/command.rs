use crate::id::Id;
use crate::params::Params;
use crate::settings::SettingValue;
use quireink_platform::RefreshMode;

/// An HTTP request of a plugin. When it is answered the kernel emits
/// `reply` with `params` plus `status` (the HTTP status; `0` when the
/// request failed, e.g. without network) and `body` (the response as text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// `GET`, `PUT`, `POST`, …
    pub method: alloc::string::String,
    pub url: alloc::string::String,
    pub headers: alloc::vec::Vec<(alloc::string::String, alloc::string::String)>,
    pub body: Option<alloc::string::String>,
    /// A file in storage to send as the body instead, e.g. a database to
    /// upload.
    pub body_from: Option<alloc::string::String>,
    /// A file in storage to write the response's body to (also large
    /// ones, e.g. a download); the reply then has no `body`.
    pub save_to: Option<alloc::string::String>,
    pub reply: Id,
    pub params: Params,
}

impl HttpRequest {
    /// A request without headers or body.
    pub fn new(method: &str, url: &str, reply: Id) -> Self {
        Self {
            method: alloc::string::String::from(method),
            url: alloc::string::String::from(url),
            headers: alloc::vec::Vec::new(),
            body: None,
            body_from: None,
            save_to: None,
            reply,
            params: Params::new(),
        }
    }
}

/// The answer to a request a plugin's server received (see
/// [`Command::Serve`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerResponse {
    /// The request's number ([`server_params::REQUEST`]).
    pub request: u32,
    pub status: u16,
    pub content_type: alloc::string::String,
    pub headers: alloc::vec::Vec<(alloc::string::String, alloc::string::String)>,
    pub body: alloc::vec::Vec<u8>,
    /// A file in storage to send as the body instead.
    pub body_from: Option<alloc::string::String>,
}

impl ServerResponse {
    /// An answer with `body` of `content_type`.
    pub fn new(request: u32, status: u16, content_type: &str, body: alloc::vec::Vec<u8>) -> Self {
        Self {
            request,
            status,
            content_type: alloc::string::String::from(content_type),
            headers: alloc::vec::Vec::new(),
            body,
            body_from: None,
        }
    }
}

/// Parameters of a server's events (see [`Command::Serve`]).
pub mod server_params {
    /// What happened: [`STARTED`], [`REQUEST`], [`FAILED`] or [`STOPPED`].
    pub const KIND: &str = "kind";
    pub const STARTED: &str = "started";
    pub const REQUEST: &str = "request";
    pub const FAILED: &str = "failed";
    pub const STOPPED: &str = "stopped";
    /// Where the server is reached, e.g. `192.168.1.20:8080`.
    pub const ADDRESS: &str = "address";
    /// Why it failed.
    pub const REASON: &str = "reason";
    /// A request's method (`GET`, `PUT`, …) and target (path and query).
    pub const METHOD: &str = "method";
    pub const TARGET: &str = "target";
    /// Where a request's body was saved, if it had one; it is removed
    /// once the request is answered, unless moved.
    pub const BODY_FILE: &str = "body-file";
}

/// Parameters of an HTTP reply event (see [`HttpRequest`]).
pub mod http_params {
    pub const STATUS: &str = "status";
    pub const BODY: &str = "body";
    /// The response's `ETag` header, if it had one.
    pub const ETAG: &str = "etag";
    /// The response's `Content-Type` header, if it had one, e.g.
    /// `text/html; charset=utf-8`.
    pub const CONTENT_TYPE: &str = "content-type";
    /// Where the answer came from, if redirected elsewhere (named apart
    /// from a request's own parameters, which come back too).
    pub const FINAL_URL: &str = "final-url";
}

/// A request from a plugin to the kernel.
///
/// Plugins never mutate kernel state directly; commands are queued and
/// applied by the kernel after the plugin returns.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Command {
    /// Shows the screen bound to a slot, passing `Params` to it. If that
    /// slot is already open, the screens above it are closed.
    OpenSlot(Id, Params),
    /// Opens a specific screen on top of the current one.
    OpenScreen(Id, Params),
    /// Closes the current screen (no-op on the root screen).
    Back,
    /// Changes a setting; invalid values are rejected with a warning.
    SetSetting(Id, SettingValue),
    /// Redraws the current screen completely.
    Redraw(RefreshMode),
    /// Notifies all subscribers of an event, then every open screen.
    Emit(Id, Params),
    /// Runs an action as if the user had triggered it (e.g. from a menu).
    RunAction(Id),
    /// Switches a plugin on or off in `quireink.ini`; applies after a
    /// restart. The kernel refuses plugins that cannot be switched off
    /// (see [`PluginInfo::can_switch_off`](crate::PluginInfo)).
    SetPluginEnabled(Id, bool),
    /// Makes an HTTP request (see [`HttpRequest`]); the answer comes as
    /// the request's reply event. Check [`Env::network`](crate::Env) first.
    Http(HttpRequest),
    /// Uses the settings stored in the file at this storage path for
    /// [scoped](crate::SettingSpec::scoped) settings, e.g. those of the
    /// book being read, while the screen that sent it stays open; `None`
    /// ends the scope.
    SetScope(Option<alloc::string::String>),
    /// Redefines a menu with exactly these entries (actions, settings or
    /// submenus), saved in `[menus]` of `quireink.ini`; `None` returns to
    /// the entries plugins contribute.
    SetMenu(Id, Option<alloc::vec::Vec<Id>>),
    /// Shows a short message for a few seconds, e.g. "Sync done", by the
    /// screen bound to [`slots::NOTICE`](crate::builtin::slots::NOTICE),
    /// over whatever is open and without taking any input. A newer
    /// message replaces it.
    Notify(alloc::string::String),
    /// Restarts QuireInk, e.g. to apply switched plugins. The shell builds
    /// a new kernel; the reading position is already saved.
    Restart,
    /// Puts text on the clipboard, on devices with one also the system's,
    /// for other apps. Long text is cut at [`MAX_CLIPBOARD`] bytes.
    SetClipboard(alloc::string::String),
    /// Serves HTTP to the local network on `port` (shells that cannot say
    /// so with a failure). Everything about the server comes as the event
    /// `events`: that it started (with its address), each request (answer
    /// with [`Command::Respond`]), that it failed or stopped; see
    /// [`server_params`]. Serve only while the user wants it: a server
    /// also stops when the screen showing as it started is closed.
    Serve { server: Id, port: u16, events: Id },
    /// Stops serving.
    StopServing(Id),
    /// Answers a request of a server.
    Respond(ServerResponse),
    /// Asks for the text on the clipboard (see
    /// [`Env::clipboard`](crate::Env)). It comes as the event `.0` with
    /// the parameters `.1` plus `text` ([`params::TEXT`](crate::builtin::params::TEXT)),
    /// empty if there is none. Read it only when the user asks to paste:
    /// some systems tell the user every time an app reads the clipboard.
    ReadClipboard(Id, Params),
}

/// The most text the clipboard holds, in bytes.
pub const MAX_CLIPBOARD: usize = 4096;

impl Command {
    /// Opens a slot without parameters.
    pub fn open_slot(slot: Id) -> Command {
        Command::OpenSlot(slot, Params::new())
    }
}
