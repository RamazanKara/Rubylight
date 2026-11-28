//! TLS connection upgrade helper (`101 Switching Protocols`) used by USB tunneling.

use super::Error;
use super::transport::{
    Abort, Cancel, Request, connect_with_cancels, io_tls_error, tls_error, tls_server_name,
    user_agent, write_timeout,
};
#[cfg(windows)]
use super::transport::abort_blocked_io;
use super::wire::{Reader, read_body, read_head};
use rustls::{ClientConnection, StreamOwned};
use socket2::{SockRef, TcpKeepalive};
use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::Mutex;
use std::time::Duration;

/// The most of an error body read from a refused upgrade.
const UPGRADE_ERROR_BYTES: usize = 64 * 1024;

/// Asks the host to switch `request`, over TLS, to `protocol`, with `fields` in the head.
/// Any answer but `101 Switching Protocols` comes back as the response it is, for its error.
pub(crate) fn upgrade(
    request: &Request<'_>,
    cancel: &Cancel,
    protocol: &str,
    fields: &[(&str, &str)],
) -> Result<Result<Upgraded, super::transport::Response>, Error> {
    let config = request
        .tls
        .clone()
        .ok_or_else(|| Error::InvalidArgument("an upgrade needs TLS".into()))?;
    let tcp = connect_with_cancels(
        request.host,
        request.port,
        request.timeouts.connect,
        None,
        &[cancel],
    )?;
    let peer = tcp.peer_addr()?;
    let id = cancel.register(&Abort::new(&tcp)?)?;
    let result = (|| {
        tcp.set_read_timeout(request.timeouts.read)?;
        tcp.set_write_timeout(Some(write_timeout(request)))?;
        tcp.set_nodelay(true)?;
        let name = tls_server_name(request.host, peer);
        let mut stream = StreamOwned::new(
            ClientConnection::new(config, name).map_err(tls_error)?,
            tcp.try_clone()?,
        );
        while stream.conn.is_handshaking() {
            stream
                .conn
                .complete_io(&mut stream.sock)
                .map_err(io_tls_error)?;
        }
        let mut head = format!(
            "GET /{} HTTP/1.1\r\nHost: {}:{}\r\nConnection: Upgrade\r\nUpgrade: {protocol}\r\nUser-Agent: {}\r\n",
            request.target,
            request.host.url_host(),
            request.port,
            user_agent()
        );
        for (name, value) in fields {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes()).map_err(io_tls_error)?;
        stream.flush().map_err(io_tls_error)?;
        let mut reader = Reader {
            stream: &mut stream,
            buffer: Vec::new(),
            eof: false,
        };
        let head = read_head(&mut reader)?;
        if head.status != 101 {
            let body = read_body(&mut reader, &head, UPGRADE_ERROR_BYTES)?;
            return Ok(Err(head.into_response(body)));
        }
        if !head
            .header("Upgrade")
            .is_some_and(|upgrade| upgrade.eq_ignore_ascii_case(protocol))
        {
            return Err(Error::Malformed(format!(
                "the host switched to {:?}, not {protocol}",
                head.header("Upgrade")
            )));
        }
        // Whatever the host sent after its head is the new protocol's.
        let early = std::mem::take(&mut reader.buffer);
        let StreamOwned { conn, sock } = stream;
        // USB devices legitimately sit idle between transfers; use TCP keepalive instead of a
        // short read timeout so dead half-open peers are still detected without breaking idle USB.
        sock.set_read_timeout(None)?;
        sock.set_write_timeout(Some(write_timeout(request)))?;
        let keepalive = TcpKeepalive::new()
            .with_time(Duration::from_secs(15))
            .with_interval(Duration::from_secs(5));
        let _ = SockRef::from(&sock).set_tcp_keepalive(&keepalive);
        Ok(Ok(Upgraded {
            socket: sock.try_clone()?,
            writing: Mutex::new(()),
            tls: Mutex::new(UpgradedTls {
                conn,
                socket: sock,
                early,
            }),
        }))
    })();
    cancel.unregister(id);
    match result {
        Err(_) if cancel.is_cancelled() => Err(Error::Cancelled),
        other => other,
    }
}

/// How long [`Upgraded::close`] waits to send the host its close_notify.
const CLOSE_NOTIFY_WAIT: Duration = Duration::from_millis(100);

/// A TLS connection the host switched to another protocol. One thread may read while others
/// write: a read waits for bytes without holding the TLS state, and a write holds it only to
/// encrypt, never while the socket takes the records.
///
/// Lock order: `writing` before `tls`; a read takes `tls` alone.
pub struct Upgraded {
    /// Waits for bytes, takes the encrypted records and closes the connection.
    socket: TcpStream,
    /// Orders writers, so records reach the socket in the order they were encrypted.
    writing: Mutex<()>,
    tls: Mutex<UpgradedTls>,
}

struct UpgradedTls {
    conn: ClientConnection,
    socket: TcpStream,
    /// Plaintext read past the response head.
    early: Vec<u8>,
}

impl Upgraded {
    fn tls(&self) -> std::sync::MutexGuard<'_, UpgradedTls> {
        self.tls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Reads what arrived, waiting for it; 0 once the host closed.
    pub fn read(&self, bytes: &mut [u8]) -> std::io::Result<usize> {
        loop {
            {
                let mut tls = self.tls();
                if !tls.early.is_empty() {
                    let n = bytes.len().min(tls.early.len());
                    bytes[..n].copy_from_slice(&tls.early[..n]);
                    tls.early.drain(..n);
                    return Ok(n);
                }
                match tls.conn.reader().read(bytes) {
                    Ok(n) => return Ok(n),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => {}
                    // The host closed without close_notify.
                    Err(e) if e.kind() == ErrorKind::UnexpectedEof => return Ok(0),
                    Err(e) => return Err(e),
                }
            }
            let mut probe = [0];
            match self.socket.peek(&mut probe) {
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
            let mut tls = self.tls();
            let UpgradedTls { conn, socket, .. } = &mut *tls;
            match conn.read_tls(socket) {
                Ok(0) => return Ok(0),
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
            conn.process_new_packets()
                .map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))?;
        }
    }

    /// Fills `bytes`; `UnexpectedEof` when the host closed first.
    pub fn read_exact(&self, mut bytes: &mut [u8]) -> std::io::Result<()> {
        while !bytes.is_empty() {
            match self.read(bytes)? {
                0 => return Err(ErrorKind::UnexpectedEof.into()),
                n => bytes = &mut bytes[n..],
            }
        }
        Ok(())
    }

    /// Sends `bytes` whole, as one write.
    pub fn write_all(&self, mut bytes: &[u8]) -> std::io::Result<()> {
        let _writing = self
            .writing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut records = Vec::new();
        while !bytes.is_empty() {
            {
                // Encrypts under the TLS lock; a blocked socket write below must not hold it,
                // or the read that would drain the host's side waits behind it.
                let mut tls = self.tls();
                let conn = &mut tls.conn;
                // rustls takes as much as its send buffer holds, 64 KiB, and none until it drained.
                let n = conn.writer().write(bytes)?;
                if n == 0 && !conn.wants_write() {
                    return Err(ErrorKind::WriteZero.into());
                }
                bytes = &bytes[n..];
                while conn.wants_write() {
                    conn.write_tls(&mut records)?;
                }
            }
            (&self.socket).write_all(&records)?;
            records.clear();
        }
        Ok(())
    }

    /// Closes the connection, waking a blocked read or write. The host first gets a TLS
    /// close_notify, a clean end, unless a write that holds the connection doesn't finish
    /// within [`CLOSE_NOTIFY_WAIT`]: the shutdown wakes it.
    pub fn close(&self) {
        let deadline = std::time::Instant::now() + CLOSE_NOTIFY_WAIT;
        fn wait<T>(
            lock: &Mutex<T>,
            deadline: std::time::Instant,
        ) -> Option<std::sync::MutexGuard<'_, T>> {
            loop {
                match lock.try_lock() {
                    Ok(guard) => break Some(guard),
                    Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                        break Some(poisoned.into_inner());
                    }
                    Err(std::sync::TryLockError::WouldBlock)
                        if std::time::Instant::now() >= deadline =>
                    {
                        break None;
                    }
                    Err(std::sync::TryLockError::WouldBlock) => {
                        std::thread::sleep(Duration::from_millis(1))
                    }
                }
            }
        }
        // The close_notify goes after any write's records, never inside them: it waits for
        // the writers, then for the TLS state.
        let writing = wait(&self.writing, deadline);
        let tls = writing.as_ref().and_then(|_| wait(&self.tls, deadline));
        if let Some(mut tls) = tls {
            let UpgradedTls { conn, socket, .. } = &mut *tls;
            conn.send_close_notify();
            // Never waits long on a host that stopped reading.
            let _ = socket.set_write_timeout(Some(CLOSE_NOTIFY_WAIT));
            while conn.wants_write() && conn.write_tls(socket).is_ok_and(|n| n > 0) {}
        }
        let _ = self.socket.shutdown(Shutdown::Both);
        #[cfg(windows)]
        abort_blocked_io(&self.socket);
    }
}
