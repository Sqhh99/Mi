//! Keeps a single running instance: later launches ask the first one to show its window,
//! handing over any files they were asked to open (e.g. "Open with Mi" in the file manager).

use std::io::{Read, Write};
use std::path::PathBuf;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Name};

const SOCKET_NAME: &str = "com.sqhh99.Mi.instance";
const ACTIVATE: &str = "ACTIVATE";
const OPEN: &str = "OPEN";

pub enum Instance {
    /// This is the first instance. Holds the listener when one could be created.
    Primary(Option<LocalSocketListener>),
    /// Another instance is running and has been handed the request.
    Secondary,
}

/// What a later launch asks of the running instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Activate,
    Open(Vec<PathBuf>),
}

impl Request {
    fn encode(&self) -> String {
        match self {
            Request::Activate => ACTIVATE.to_string(),
            Request::Open(paths) => {
                let mut message = OPEN.to_string();
                for path in paths {
                    message.push('\n');
                    message.push_str(&path.to_string_lossy());
                }
                message
            }
        }
    }

    fn decode(message: &str) -> Option<Self> {
        let mut lines = message.split('\n');
        match lines.next()? {
            ACTIVATE => Some(Request::Activate),
            OPEN => Some(Request::Open(lines.filter(|line| !line.is_empty()).map(PathBuf::from).collect())),
            _ => None,
        }
    }
}

/// One instance per settings directory: a run with `MI_CONFIG_DIR` (see
/// `model::settings::config_dir`) doesn't hand over to the everyday instance.
fn socket_name() -> std::io::Result<Name<'static>> {
    let name = match std::env::var_os("MI_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            dir.hash(&mut hasher);
            format!("{SOCKET_NAME}.{:016x}", hasher.finish())
        }
        None => SOCKET_NAME.to_string(),
    };
    name.to_ns_name::<GenericNamespaced>()
}

pub fn acquire(request: &Request) -> Instance {
    let Ok(name) = socket_name() else {
        return Instance::Primary(None);
    };
    if let Ok(mut stream) = LocalSocketStream::connect(name.clone()) {
        if let Err(err) = stream.write_all(request.encode().as_bytes()) {
            log::warn!("cannot signal running instance: {err}");
        }
        return Instance::Secondary;
    }
    match ListenerOptions::new().name(name).create_sync() {
        Ok(listener) => Instance::Primary(Some(listener)),
        Err(err) => {
            log::warn!("single-instance listener unavailable: {err}");
            Instance::Primary(None)
        }
    }
}

/// Calls `on_request` (on a background thread) whenever another launch asks for something.
pub fn listen(listener: LocalSocketListener, on_request: impl Fn(Request) + Send + 'static) {
    std::thread::Builder::new()
        .name("single-instance".into())
        .spawn(move || {
            for connection in listener.incoming() {
                let Ok(mut connection) = connection else { continue };
                let mut message = String::new();
                if connection.read_to_string(&mut message).is_ok()
                    && let Some(request) = Request::decode(&message)
                {
                    on_request(request);
                }
            }
        })
        .expect("failed to spawn single-instance thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip() {
        for request in [
            Request::Activate,
            Request::Open(vec![PathBuf::from("C:/Music/稻香.flac"), PathBuf::from("/tmp/a b.mp3")]),
            Request::Open(Vec::new()),
        ] {
            assert_eq!(Request::decode(&request.encode()), Some(request));
        }
        assert_eq!(Request::decode("nonsense"), None);
    }
}
