use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use tungstenite::{Error, Message, error::ProtocolError};

use super::{
    snapshot::Snapshot,
    socket::{self, REQUEST_TIMEOUT, Socket},
};

pub(super) const PROTOCOL: &str = "rex.data.terminal.v1";

#[derive(Debug, PartialEq, Eq)]
struct Welcome {
    token: [u8; 16],
    retention: Duration,
    nonce: u64,
}

fn hello(ticket: &str) -> io::Result<[u8; 17]> {
    let token = URL_SAFE_NO_PAD.decode(ticket).map_err(io::Error::other)?;
    let mut hello = [0; 17];
    if token.len() != 16 {
        return Err(socket::invalid("Invalid Rex terminal data ticket length"));
    }
    hello[0] = 0x01;
    hello[1..].copy_from_slice(&token);
    Ok(hello)
}

fn resume(token: [u8; 16], received: u64) -> [u8; 25] {
    let mut resume = [0; 25];
    resume[0] = 0x02;
    resume[1..17].copy_from_slice(&token);
    resume[17..].copy_from_slice(&received.to_le_bytes());
    resume
}

fn welcome(bytes: &[u8]) -> io::Result<Welcome> {
    if let [0x82, reason] = bytes {
        return Err(socket::invalid(format!(
            "Rex data handshake rejected (reason {reason})"
        )));
    }
    let Ok(bytes): Result<&[u8; 32], _> = bytes.try_into() else {
        return Err(socket::invalid("Invalid Rex data welcome length"));
    };
    let [
        0x81,
        token @ ..,
        0,
        0,
        0,
        d0,
        d1,
        d2,
        d3,
        n0,
        n1,
        n2,
        n3,
        n4,
        n5,
        n6,
        n7,
    ] = *bytes
    else {
        return Err(socket::invalid("Invalid Rex data welcome"));
    };
    let nonce = u64::from_le_bytes([n0, n1, n2, n3, n4, n5, n6, n7]);
    if nonce == 0 {
        return Err(socket::invalid("Invalid Rex data welcome nonce"));
    }
    Ok(Welcome {
        token,
        retention: Duration::from_millis(u32::from_le_bytes([d0, d1, d2, d3]).into()),
        nonce,
    })
}

fn handshake(endpoint: &Path, request: &[u8]) -> io::Result<(Socket, Welcome)> {
    let mut socket = socket::connect(endpoint, "/api/data", PROTOCOL, 16 * 1024 * 1024)?;
    socket
        .send(Message::Binary(request.to_vec().into()))
        .map_err(io::Error::other)?;
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    while Instant::now() < deadline {
        match socket.read() {
            Ok(Message::Binary(bytes)) => return Ok((socket, welcome(&bytes)?)),
            Ok(Message::Ping(_)) => socket.flush().map_err(io::Error::other)?,
            Ok(Message::Pong(_)) => {}
            Ok(_) => return Err(socket::invalid("Expected Rex binary data welcome")),
            Err(error) if socket::idle(&error) => {}
            Err(error) => return Err(io::Error::other(error)),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "Rex data handshake timed out",
    ))
}

pub(super) struct Data {
    endpoint: PathBuf,
    socket: Socket,
    welcome: Welcome,
    received: u64,
}

pub(super) enum Frame {
    Idle,
    Output(tungstenite::Bytes),
    Closed(&'static str),
}

impl Data {
    pub(super) fn connect(endpoint: PathBuf, ticket: &str) -> io::Result<Self> {
        let (socket, welcome) = handshake(&endpoint, &hello(ticket)?)?;
        Ok(Self {
            endpoint,
            socket,
            welcome,
            received: 0,
        })
    }

    pub(super) fn nonce(&self) -> u64 {
        self.welcome.nonce
    }

    pub(super) fn read_snapshot(
        &mut self,
        stopping: &AtomicBool,
    ) -> io::Result<(Vec<u8>, Vec<u8>)> {
        let mut snapshot = Snapshot::new();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !stopping.load(Ordering::Relaxed) && Instant::now() < deadline {
            match self.read(stopping)? {
                Frame::Output(bytes) => {
                    if snapshot.push(&bytes)? {
                        return Ok(snapshot.split());
                    }
                }
                Frame::Closed(reason) => return Err(io::Error::other(reason)),
                Frame::Idle => {}
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Rex terminal snapshot interrupted or timed out",
        ))
    }

    pub(super) fn read(&mut self, stopping: &AtomicBool) -> io::Result<Frame> {
        match self.socket.read() {
            Ok(Message::Binary(bytes)) => {
                self.received += bytes.len() as u64;
                Ok(Frame::Output(bytes))
            }
            Ok(Message::Ping(_)) => {
                self.socket.flush().map_err(io::Error::other)?;
                Ok(Frame::Idle)
            }
            Ok(Message::Close(frame)) => {
                let _ = self.socket.flush();
                match frame.as_ref().map(|frame| u16::from(frame.code)) {
                    None | Some(1000) => Ok(Frame::Closed("Rex terminal stream closed")),
                    Some(4000) => Ok(Frame::Closed("Rex block closed")),
                    Some(1006) => self.reconnect(stopping),
                    Some(1001)
                        if frame.is_some_and(|frame| frame.reason == "keepalive timeout") =>
                    {
                        self.reconnect(stopping)
                    }
                    Some(code) => Err(io::Error::other(format!(
                        "Rex terminal stream closed ({code})"
                    ))),
                }
            }
            Ok(_) => Ok(Frame::Idle),
            Err(error) if socket::idle(&error) => Ok(Frame::Idle),
            Err(
                Error::Io(_)
                | Error::ConnectionClosed
                | Error::Protocol(ProtocolError::ResetWithoutClosingHandshake),
            ) => self.reconnect(stopping),
            Err(error) => Err(io::Error::other(error)),
        }
    }

    fn reconnect(&mut self, stopping: &AtomicBool) -> io::Result<Frame> {
        let deadline = Instant::now() + self.welcome.retention;
        let request = resume(self.welcome.token, self.received);
        let mut failure = io::Error::other("Rex terminal stream lost its connection");
        for attempt in 0..3 {
            if stopping.load(Ordering::Relaxed) || Instant::now() >= deadline {
                break;
            }
            if attempt > 0 {
                thread::sleep(Duration::from_millis(100));
            }
            match handshake(&self.endpoint, &request) {
                Ok((socket, welcome)) => {
                    if welcome.nonce != self.welcome.nonce {
                        return Err(socket::invalid(
                            "Rex changed the nonce of the resumed terminal stream",
                        ));
                    }
                    self.socket = socket;
                    self.welcome = welcome;
                    return Ok(Frame::Idle);
                }
                Err(error) => failure = error,
            }
        }
        Err(failure)
    }
}

impl Drop for Data {
    fn drop(&mut self) {
        socket::disconnect(&mut self.socket);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_handshake_byte_layout() -> io::Result<()> {
        let ticket = hello("AAECAwQFBgcICQoLDA0ODw")?;
        assert_eq!(
            ticket,
            [1, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
        );
        assert!(hello("abc").is_err());

        let token = [42; 16];
        let wire = resume(token, 0x0123_4567_89ab_cdef);
        assert_eq!(wire[0], 2);
        assert_eq!(&wire[1..17], &token);
        assert_eq!(
            &wire[17..],
            &[0xef, 0xcd, 0xab, 0x89, 0x67, 0x45, 0x23, 0x01]
        );

        let mut reply = [0; 32];
        reply[0] = 0x81;
        reply[1..17].copy_from_slice(&token);
        reply[20..24].copy_from_slice(&60_000u32.to_le_bytes());
        reply[24..].copy_from_slice(&9u64.to_le_bytes());
        assert_eq!(
            welcome(&reply)?,
            Welcome {
                token,
                retention: Duration::from_secs(60),
                nonce: 9
            }
        );
        assert!(welcome(&reply[..31]).is_err());
        assert!(welcome(&[0x82, 3]).is_err());
        reply[24] = 0;
        assert!(welcome(&reply).is_err());
        Ok(())
    }
}
