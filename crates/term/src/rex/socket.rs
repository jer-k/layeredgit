use std::{
    env,
    ffi::OsStr,
    io,
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    time::Duration,
};

use percent_encoding::percent_decode;
use tungstenite::{
    Error, WebSocket,
    client::IntoClientRequest,
    handshake::client::Request,
    http::{HeaderValue, header::SEC_WEBSOCKET_PROTOCOL},
    protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
};

pub(super) type Socket = WebSocket<UnixStream>;
pub(super) const POLL: Duration = Duration::from_millis(100);
pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn endpoint() -> io::Result<PathBuf> {
    if let Some(value) = env::var_os("REX_SERVER").filter(|value| !value.is_empty()) {
        return parse_endpoint(&value);
    }
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .map(|home| home.join("Library/Application Support/rex/server.sock"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "No home directory for Rex socket"))
}

fn parse_endpoint(value: &OsStr) -> io::Result<PathBuf> {
    let bytes = value.as_bytes();
    if let Some(path) = bytes.strip_prefix(b"unix://") {
        let path = path.strip_prefix(b"localhost").unwrap_or(path);
        if path.starts_with(b"/") && !path.contains(&b'?') && !path.contains(&b'#') {
            let path: Vec<u8> = percent_decode(path).collect();
            if !path.contains(&0) {
                return Ok(PathBuf::from(std::ffi::OsString::from_vec(path)));
            }
        }
    } else if !bytes.windows(3).any(|bytes| bytes == b"://")
        && !bytes.contains(&0)
        && !bytes.is_empty()
    {
        return Ok(PathBuf::from(value));
    }
    Err(invalid("REX_SERVER must name a Unix socket"))
}

pub(super) fn connect(
    endpoint: &Path,
    path: &str,
    protocol: &'static str,
    max_message_size: usize,
) -> io::Result<Socket> {
    let stream = UnixStream::connect(endpoint)?;
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(REQUEST_TIMEOUT))?;
    let mut request: Request = format!("ws://localhost{path}")
        .into_client_request()
        .map_err(io::Error::other)?;
    request
        .headers_mut()
        .insert(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(protocol));
    let config = WebSocketConfig::default()
        .max_frame_size(Some(max_message_size))
        .max_message_size(Some(max_message_size));
    let (socket, response) = tungstenite::client::client_with_config(request, stream, Some(config))
        .map_err(io::Error::other)?;
    if response
        .headers()
        .get(SEC_WEBSOCKET_PROTOCOL)
        .is_none_or(|header| header != protocol)
    {
        return Err(invalid("Rex WebSocket subprotocol mismatch"));
    }
    socket.get_ref().set_read_timeout(Some(POLL))?;
    Ok(socket)
}

pub(super) fn disconnect(socket: &mut Socket) {
    let _ = socket.close(Some(CloseFrame {
        code: CloseCode::Normal,
        reason: "".into(),
    }));
}

pub(super) fn idle(error: &Error) -> bool {
    matches!(error, Error::Io(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut))
}

pub(super) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_socket_paths_and_decodes_only_urls() -> io::Result<()> {
        for (input, expected) in [
            ("/tmp/my socket", "/tmp/my socket"),
            ("rex.sock", "rex.sock"),
            ("/tmp/socket%20name", "/tmp/socket%20name"),
            ("unix:///tmp/my%20socket", "/tmp/my socket"),
            ("unix://localhost/tmp/rex.sock", "/tmp/rex.sock"),
            ("unix:///tmp/socket%3F%23", "/tmp/socket?#"),
        ] {
            assert_eq!(parse_endpoint(OsStr::new(input))?, Path::new(expected));
        }
        for input in [
            "",
            "https://localhost/rex",
            "unix://other/tmp/rex",
            "unix:///tmp/%00",
            "unix:///tmp/socket?x=1",
        ] {
            assert!(parse_endpoint(OsStr::new(input)).is_err(), "{input}");
        }
        Ok(())
    }
}
