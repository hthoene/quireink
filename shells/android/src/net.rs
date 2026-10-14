//! HTTP for Android: each request runs on a thread of its own through
//! Java's `HttpURLConnection` (the system's TLS and certificates, no extra
//! library), and the event loop is woken when an answer has arrived.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use android_activity::{AndroidApp, AndroidAppWaker};
use jni::JNIEnv;
use jni::JavaVM;
use jni::objects::{JObject, JValue};
use quireink_core::{HttpCall, HttpResponse};

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
        ..HttpResponse::default()
    }
}

/// Redirects followed at most, also from `http` to `https` (which Java
/// does not follow by itself).
const MAX_REDIRECTS: u32 = 10;

fn perform(vm: &JavaVM, call: &HttpCall) -> HttpResponse {
    let Ok(mut env) = vm.attach_current_thread() else {
        return failed(call, "cannot attach to the Java VM");
    };
    let result = env.with_local_frame(64, |env| request(env, call));
    match result {
        Ok(response) => response,
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

/// A header of the answer of `conn`, if it has it.
fn header(
    env: &mut JNIEnv<'_>,
    conn: &JObject<'_>,
    name: &str,
) -> jni::errors::Result<Option<String>> {
    let name = env.new_string(name)?;
    let value = env
        .call_method(
            conn,
            "getHeaderField",
            "(Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(&name)],
        )?
        .l()?;
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(String::from(
        env.get_string(&jni::objects::JString::from(value))?,
    )))
}

fn request(env: &mut JNIEnv<'_>, call: &HttpCall) -> jni::errors::Result<HttpResponse> {
    let address = env.new_string(&call.url)?;
    let mut url = env.new_object(
        "java/net/URL",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&address)],
    )?;
    let mut redirects = 0;
    loop {
        let conn = connect(env, &url, call)?;
        let status = env.call_method(&conn, "getResponseCode", "()I", &[])?.i()?;
        let location = header(env, &conn, "Location")?;
        if matches!(status, 301 | 302 | 303 | 307 | 308)
            && redirects < MAX_REDIRECTS
            && call.body.is_none()
            && let Some(location) = location
        {
            // Relative to the address it came from.
            let location = env.new_string(&location)?;
            url = env.new_object(
                "java/net/URL",
                "(Ljava/net/URL;Ljava/lang/String;)V",
                &[JValue::Object(&url), JValue::Object(&location)],
            )?;
            env.call_method(&conn, "disconnect", "()V", &[])?;
            redirects += 1;
            continue;
        }
        let mut response = answer(env, &conn, call, status)?;
        if redirects > 0 {
            let text = env
                .call_method(&url, "toString", "()Ljava/lang/String;", &[])?
                .l()?;
            response.url = Some(String::from(
                env.get_string(&jni::objects::JString::from(text))?,
            ));
        }
        return Ok(response);
    }
}

/// Opens `url` and sends the request.
fn connect<'a>(
    env: &mut JNIEnv<'a>,
    url: &JObject<'_>,
    call: &HttpCall,
) -> jni::errors::Result<JObject<'a>> {
    let conn = env
        .call_method(url, "openConnection", "()Ljava/net/URLConnection;", &[])?
        .l()?;
    let method = env.new_string(&call.method)?;
    env.call_method(
        &conn,
        "setRequestMethod",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&method)],
    )?;
    env.call_method(&conn, "setConnectTimeout", "(I)V", &[JValue::Int(15_000)])?;
    env.call_method(&conn, "setReadTimeout", "(I)V", &[JValue::Int(60_000)])?;
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
    Ok(conn)
}

/// Reads the answer of `conn`.
fn answer(
    env: &mut JNIEnv<'_>,
    conn: &JObject<'_>,
    call: &HttpCall,
    status: i32,
) -> jni::errors::Result<HttpResponse> {
    let etag = header(env, conn, "ETag")?;
    let content_type = header(env, conn, "Content-Type")?;
    // Errors come with a body too, e.g. the server's reason.
    let stream = if status >= 400 {
        "getErrorStream"
    } else {
        "getInputStream"
    };
    let stream = env
        .call_method(conn, stream, "()Ljava/io/InputStream;", &[])?
        .l()?;
    let mut body = Vec::new();
    if !stream.is_null() {
        let buffer = env.new_byte_array(8192)?;
        loop {
            let n = env
                .call_method(&stream, "read", "([B)I", &[JValue::Object(&buffer)])?
                .i()?;
            if n < 0 || body.len() >= call.max_response {
                break;
            }
            let mut chunk = vec![0i8; n as usize];
            env.get_byte_array_region(&buffer, 0, &mut chunk)?;
            body.extend(chunk.iter().map(|&b| b as u8));
        }
        env.call_method(&stream, "close", "()V", &[])?;
    }
    env.call_method(conn, "disconnect", "()V", &[])?;
    Ok(HttpResponse {
        id: call.id,
        status: status.clamp(0, i32::from(u16::MAX)) as u16,
        body,
        etag,
        content_type,
        url: None,
    })
}
