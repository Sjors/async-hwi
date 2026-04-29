//! Transport abstraction for Coldcard communication.
//!
//! The Coldcard hardware speaks a custom 64-byte framed protocol over USB HID.
//! The simulator (`coldcard-mpy`) speaks the same protocol but over an
//! `AF_UNIX SOCK_DGRAM` socket at `/tmp/ckcc-simulator.sock`, where each
//! datagram carries one frame minus the leading HID report ID byte.
//!
//! The [`Transport`] trait abstracts these two backends so that the
//! framing/encryption layers in [`crate`] can be reused unchanged.

use std::io;

/// Errors returned by a [`Transport`] implementation.
#[derive(Debug)]
pub enum TransportError {
    Hid(hidapi::HidError),
    Io(io::Error),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Hid(e) => write!(f, "hid: {e}"),
            TransportError::Io(e) => write!(f, "io: {e}"),
        }
    }
}

impl std::error::Error for TransportError {}

impl From<hidapi::HidError> for TransportError {
    fn from(e: hidapi::HidError) -> Self {
        TransportError::Hid(e)
    }
}

impl From<io::Error> for TransportError {
    fn from(e: io::Error) -> Self {
        TransportError::Io(e)
    }
}

/// Abstraction over the I/O channel between the host and a Coldcard.
///
/// Implementations must preserve the Coldcard wire framing:
/// * `write` is given a 65-byte buffer where byte 0 is the HID report id
///   (always 0x00) and bytes 1..65 are the frame payload. Implementations
///   are free to drop the leading byte if their underlying transport does
///   not need it (e.g. Unix sockets).
/// * `read`/`read_timeout` must place a single 64-byte frame in `buf` and
///   return its length (always 64 on success).
pub trait Transport: Send {
    /// Sends a 65-byte HID-style buffer (`buf[0]` is the report id).
    fn write(&mut self, buf: &[u8]) -> Result<usize, TransportError>;

    /// Blocks until a 64-byte frame is available.
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError>;

    /// Reads a 64-byte frame waiting up to `timeout_ms` milliseconds. Returns
    /// `Ok(0)` if the timeout elapsed.
    fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, TransportError>;

    /// Sets blocking mode on the underlying handle. Best effort: backends
    /// that are inherently blocking may simply ignore this call.
    fn set_blocking_mode(&mut self, blocking: bool) -> Result<(), TransportError>;
}

impl Transport for hidapi::HidDevice {
    fn write(&mut self, buf: &[u8]) -> Result<usize, TransportError> {
        Ok(hidapi::HidDevice::write(self, buf)?)
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        Ok(hidapi::HidDevice::read(self, buf)?)
    }

    fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, TransportError> {
        Ok(hidapi::HidDevice::read_timeout(self, buf, timeout_ms)?)
    }

    fn set_blocking_mode(&mut self, blocking: bool) -> Result<(), TransportError> {
        Ok(hidapi::HidDevice::set_blocking_mode(self, blocking)?)
    }
}

/// Connects to a running Coldcard simulator over its `AF_UNIX SOCK_DGRAM`
/// socket. The default path used by `coldcard-mpy --headless` is
/// `/tmp/ckcc-simulator.sock`.
#[cfg(unix)]
pub struct UnixDatagramTransport {
    sock: std::os::unix::net::UnixDatagram,
    server_path: std::path::PathBuf,
    client_path: std::path::PathBuf,
}

#[cfg(unix)]
impl UnixDatagramTransport {
    /// Connects to the simulator socket at `server_path`. A unique client
    /// socket is bound under `/tmp` so the simulator can address replies.
    pub fn connect(server_path: impl AsRef<std::path::Path>) -> Result<Self, TransportError> {
        use std::os::unix::net::UnixDatagram;

        let server_path = server_path.as_ref().to_path_buf();
        // Build a unique client socket path: /tmp/ckcc-client-<pid>-<nanos>.sock
        // matching the convention used by Coldcard's Python CLI so the simulator
        // can pick a stable reply address.
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let client_path = std::path::PathBuf::from(format!("/tmp/ckcc-client-{pid}-{nanos}.sock"));
        let _ = std::fs::remove_file(&client_path);

        let sock = UnixDatagram::bind(&client_path)?;
        sock.connect(&server_path)?;

        Ok(Self {
            sock,
            server_path,
            client_path,
        })
    }

    /// Path of the simulator socket this transport is connected to.
    pub fn server_path(&self) -> &std::path::Path {
        &self.server_path
    }
}

#[cfg(unix)]
impl Drop for UnixDatagramTransport {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.client_path);
    }
}

#[cfg(unix)]
impl Transport for UnixDatagramTransport {
    fn write(&mut self, buf: &[u8]) -> Result<usize, TransportError> {
        // The simulator expects 64-byte datagrams without the leading HID
        // report id. Strip byte 0 if the buffer is the typical 65 bytes.
        let payload = if buf.len() == 65 { &buf[1..] } else { buf };
        let n = self.sock.send(payload)?;
        // Return the original buffer length so callers see the same accounting
        // they would get from the HID backend.
        Ok(n + (buf.len() - payload.len()))
    }

    fn read(&mut self, buf: &mut [u8]) -> Result<usize, TransportError> {
        // Use a long blocking timeout: the simulator can take time to
        // produce signed PSBT responses when polled.
        self.sock.set_read_timeout(None)?;
        Ok(self.sock.recv(buf)?)
    }

    fn read_timeout(&mut self, buf: &mut [u8], timeout_ms: i32) -> Result<usize, TransportError> {
        if timeout_ms <= 0 {
            self.sock.set_read_timeout(None)?;
        } else {
            self.sock
                .set_read_timeout(Some(std::time::Duration::from_millis(timeout_ms as u64)))?;
        }
        match self.sock.recv(buf) {
            Ok(n) => Ok(n),
            Err(e)
                if e.kind() == io::ErrorKind::WouldBlock || e.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(0)
            }
            Err(e) => Err(e.into()),
        }
    }

    fn set_blocking_mode(&mut self, blocking: bool) -> Result<(), TransportError> {
        self.sock.set_nonblocking(!blocking)?;
        Ok(())
    }
}
