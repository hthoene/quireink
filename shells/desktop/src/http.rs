//! HTTP for the emulator: each request runs the system's `curl` on a
//! thread of its own (Windows, macOS and Linux ship it), so the emulator
//! needs no TLS library.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

use quireink_core::{HttpCall, HttpResponse, MAX_RESPONSE};

/// Requests in flight and their answers.
#[derive(Debug)]
pub struct Http {
    sender: Sender<HttpResponse>,
    receiver: Receiver<HttpResponse>,
    pub in_flight: usize,
}

impl Http {
    pub fn new() -> Self {
        let (sender, receiver) = channel();
        Self {
            sender,
            receiver,
            in_flight: 0,
        }
    }

    /// Starts a request.
    pub fn start(&mut self, call: HttpCall) {
        let sender = self.sender.clone();
        self.in_flight += 1;
        thread::spawn(move || {
            let _ = sender.send(run(call));
        });
    }

    /// An answer that has arrived, if any; waits up to `wait`.
    pub fn next(&mut self, wait: Option<Duration>) -> Option<HttpResponse> {
        let response = match wait {
            Some(wait) => self.receiver.recv_timeout(wait).ok(),
            None => self.receiver.try_recv().ok(),
        };
        if response.is_some() {
            self.in_flight -= 1;
        }
        response
    }
}

/// Makes the request with `curl`; status 0 with the reason if it fails.
fn run(call: HttpCall) -> HttpResponse {
    let failed = |reason: String| HttpResponse {
        id: call.id,
        status: 0,
        body: reason.into_bytes(),
    };
    let mut command = Command::new("curl");
    command
        .args(["-sS", "--max-time", "20", "-X", &call.method])
        .args(["-w", "\n%{http_code}"]);
    for (name, value) in &call.headers {
        command.args(["-H", &format!("{name}: {value}")]);
    }
    if call.body.is_some() {
        command.args(["--data-binary", "@-"]);
    }
    command
        .arg(&call.url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window for each request.
        command.creation_flags(0x0800_0000);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return failed(format!("curl: {e}")),
    };
    if let (Some(body), Some(mut stdin)) = (&call.body, child.stdin.take()) {
        let _ = stdin.write_all(body);
    }
    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(e) => return failed(format!("curl: {e}")),
    };
    if !output.status.success() {
        return failed(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    // The body, then a line with the status (`-w`).
    let mut out = output.stdout;
    let split = out.iter().rposition(|&b| b == b'\n').unwrap_or(0);
    let status = String::from_utf8_lossy(&out[split..])
        .trim()
        .parse()
        .unwrap_or(0);
    out.truncate(split.min(MAX_RESPONSE));
    HttpResponse {
        id: call.id,
        status,
        body: out,
    }
}
