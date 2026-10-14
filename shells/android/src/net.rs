//! HTTP for Android: each request runs on a thread of its own through
//! Java's `HttpURLConnection` (the system's TLS and certificates, no extra
//! library), and the event loop is woken when an answer has arrived.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use android_activity::{AndroidApp, AndroidAppWaker};
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{JObject, JValue};
use quireink_core::{HttpCall, HttpResponse, MAX_RESPONSE};

/// Requests in flight and their answers.
pub struct Net {
    vm: usize,
    waker: AndroidAppWaker,
    sender: Sender<HttpResponse>,
    receiver: Receiver<HttpResponse>,
}

impl Net {
    pub fn new(app: &AndroidApp) -> Self {
        let (sender, receiver) = channel();
        Self {
            vm: app.vm_as_ptr() as usize,
            waker: app.create_waker(),
            sender,
            receiver,
        }
    }

    /// Starts a request.
    pub fn start(&self, call: HttpCall) {
        let (vm, waker, sender) = (self.vm, self.waker.clone(), self.sender.clone());
        thread::spawn(move || {
            // SAFETY: the process's Java VM, valid while the app runs.
            let response = match unsafe { JavaVM::from_raw(vm as *mut jni::sys::JavaVM) } {
                Ok(vm) => perform(&vm, &call),
                Err(_) => failed(&call, "no Java VM"),
            };
            let _ = sender.send(response);
            waker.wake();
        });
    }

    /// An answer that has arrived, if any.
    pub fn next(&self) -> Option<HttpResponse> {
        self.receiver.try_recv().ok()
    }
}

fn failed(call: &HttpCall, reason: &str) -> HttpResponse {
    HttpResponse {
        id: call.id,
        status: 0,
        body: reason.as_bytes().to_vec(),
    }
}

fn perform(vm: &JavaVM, call: &HttpCall) -> HttpResponse {
    let Ok(mut env) = vm.attach_current_thread() else {
        return failed(call, "cannot attach to the Java VM");
    };
    let result = env.with_local_frame(64, |env| request(env, call));
    match result {
        Ok((status, body)) => HttpResponse {
            id: call.id,
            status,
            body,
        },
        Err(e) => {
            // E.g. no network or an unknown host (an IOException).
            if env.exception_check().unwrap_or(false) {
                let _ = env.exception_clear();
            }
            log::warn!("{} {}: {e}", call.method, call.url);
            failed(call, "request failed")
        }
    }
}

fn request(env: &mut JNIEnv<'_>, call: &HttpCall) -> jni::errors::Result<(u16, Vec<u8>)> {
    let url = env.new_string(&call.url)?;
    let url = env.new_object(
        "java/net/URL",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&url)],
    )?;
    let conn = env
        .call_method(&url, "openConnection", "()Ljava/net/URLConnection;", &[])?
        .l()?;
    let method = env.new_string(&call.method)?;
    env.call_method(
        &conn,
        "setRequestMethod",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&method)],
    )?;
    env.call_method(&conn, "setConnectTimeout", "(I)V", &[JValue::Int(15_000)])?;
    env.call_method(&conn, "setReadTimeout", "(I)V", &[JValue::Int(20_000)])?;
    for (name, value) in &call.headers {
        let (name, value) = (env.new_string(name)?, env.new_string(value)?);
        env.call_method(
            &conn,
            "setRequestProperty",
            "(Ljava/lang/String;Ljava/lang/String;)V",
            &[JValue::Object(&name), JValue::Object(&value)],
        )?;
    }
    if let Some(body) = &call.body {
        env.call_method(&conn, "setDoOutput", "(Z)V", &[JValue::Bool(1)])?;
        let out = env
            .call_method(&conn, "getOutputStream", "()Ljava/io/OutputStream;", &[])?
            .l()?;
        let bytes: JObject<'_> = env.byte_array_from_slice(body)?.into();
        env.call_method(&out, "write", "([B)V", &[JValue::Object(&bytes)])?;
        env.call_method(&out, "close", "()V", &[])?;
    }
    let status = env.call_method(&conn, "getResponseCode", "()I", &[])?.i()?;
    // Errors come with a body too, e.g. the server's reason.
    let stream = if status >= 400 {
        "getErrorStream"
    } else {
        "getInputStream"
    };
    let stream = env
        .call_method(&conn, stream, "()Ljava/io/InputStream;", &[])?
        .l()?;
    let mut body = Vec::new();
    if !stream.is_null() {
        let buffer = env.new_byte_array(8192)?;
        loop {
            let n = env
                .call_method(&stream, "read", "([B)I", &[JValue::Object(&buffer)])?
                .i()?;
            if n < 0 || body.len() >= MAX_RESPONSE {
                break;
            }
            let mut chunk = vec![0i8; n as usize];
            env.get_byte_array_region(&buffer, 0, &mut chunk)?;
            body.extend(chunk.iter().map(|&b| b as u8));
        }
        env.call_method(&stream, "close", "()V", &[])?;
    }
    env.call_method(&conn, "disconnect", "()V", &[])?;
    Ok((status.clamp(0, i32::from(u16::MAX)) as u16, body))
}
