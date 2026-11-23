use std::{io, path::Path, time::Instant};

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use tungstenite::Message;

use super::socket::{self, REQUEST_TIMEOUT, Socket};

const PROTOCOL: &str = "rex.control.v1";
const MIN_REVISION: u16 = 16;
const MAX_REVISION: u16 = 18;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Incoming {
    Welcome {
        protocol: String,
        api_revision: u16,
        limits: Limits,
    },
    Response {
        id: String,
        #[serde(default)]
        payload: Value,
        error: Option<MethodError>,
    },
    #[serde(other)]
    Event,
}

#[derive(Deserialize)]
struct Limits {
    max_message_bytes: usize,
}

#[derive(Deserialize)]
struct MethodError {
    code: String,
    message: String,
}

pub(super) struct Control {
    socket: Socket,
    id: u64,
    max_message_bytes: usize,
}

impl Control {
    pub(super) fn connect(endpoint: &Path) -> io::Result<Self> {
        let mut control = Self {
            socket: socket::connect(endpoint, "/api/control", PROTOCOL, 2 * 1024 * 1024)?,
            id: 0,
            max_message_bytes: usize::MAX,
        };
        control.send(&json!({
            "type": "hello",
            "api_revision": MAX_REVISION,
            "api_min_revision": MIN_REVISION,
            "client": {
                "kind": "app",
                "name": "layeredgit",
                "version": env!("CARGO_PKG_VERSION"),
                "platform": "macos"
            }
        }))?;
        let incoming = control.receive_before(Instant::now() + REQUEST_TIMEOUT)?;
        let Incoming::Welcome {
            protocol,
            api_revision,
            limits,
        } = incoming
        else {
            return Err(socket::invalid("Rex did not send a control welcome"));
        };
        if protocol != PROTOCOL || !(MIN_REVISION..=MAX_REVISION).contains(&api_revision) {
            return Err(socket::invalid(format!(
                "Unsupported Rex protocol {protocol} revision {api_revision}"
            )));
        }
        control.max_message_bytes = limits.max_message_bytes;
        Ok(control)
    }

    pub(super) fn call<T: DeserializeOwned>(
        &mut self,
        method: &str,
        payload: &Value,
    ) -> io::Result<T> {
        self.call_before(method, payload, Instant::now() + REQUEST_TIMEOUT)
    }

    pub(super) fn call_before<T: DeserializeOwned>(
        &mut self,
        method: &str,
        payload: &Value,
        deadline: Instant,
    ) -> io::Result<T> {
        self.id += 1;
        let id = self.id.to_string();
        self.send(&json!({"type": "request", "id": id, "method": method, "payload": payload}))?;
        loop {
            if let Incoming::Response {
                id: response_id,
                payload,
                error,
            } = self.receive_before(deadline)?
                && response_id == id
            {
                if let Some(error) = error {
                    return Err(io::Error::other(format!(
                        "{method}: {} ({})",
                        error.message, error.code
                    )));
                }
                return serde_json::from_value(payload).map_err(|error| {
                    socket::invalid(format!("Invalid Rex {method} response: {error}"))
                });
            }
        }
    }

    fn send(&mut self, payload: &Value) -> io::Result<()> {
        let body = payload.to_string();
        if body.len() > self.max_message_bytes {
            return Err(socket::invalid(
                "Rex control request exceeds the server message limit",
            ));
        }
        self.socket
            .send(Message::Text(body.into()))
            .map_err(io::Error::other)
    }

    fn receive_before(&mut self, deadline: Instant) -> io::Result<Incoming> {
        while Instant::now() < deadline {
            if let Some(message) = self.read()? {
                return Ok(message);
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Rex control request timed out",
        ))
    }

    // Reading while there are no requests lets tungstenite answer WebSocket keepalives.
    pub(super) fn idle(&mut self) -> io::Result<()> {
        for _ in 0..32 {
            if self.read()?.is_none() {
                break;
            }
        }
        Ok(())
    }

    fn read(&mut self) -> io::Result<Option<Incoming>> {
        match self.socket.read() {
            Ok(Message::Text(text)) => Ok(Some(serde_json::from_str(text.as_str())?)),
            Ok(Message::Close(_)) => Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Rex control connection closed",
            )),
            Ok(Message::Ping(_)) => {
                self.socket.flush().map_err(io::Error::other)?;
                Ok(None)
            }
            Ok(_) => Ok(None),
            Err(error) if socket::idle(&error) => Ok(None),
            Err(error) => Err(io::Error::other(error)),
        }
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        socket::disconnect(&mut self.socket);
    }
}
