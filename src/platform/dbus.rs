//! A minimal D-Bus client: SASL EXTERNAL over a Unix socket plus just enough
//! of the wire format for notifications and a tray icon.
//!
//! This replaces zbus/ksni, which cost ~800 KB of code and four threads for
//! what amounts to a handful of small messages.

use std::io::{self, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, PoisonError};
use std::{env, str};

pub const BUS_NAME: &str = "org.freedesktop.DBus";
pub const BUS_PATH: &str = "/org/freedesktop/DBus";

/// The spec's upper bound for a single message.
const MAX_MESSAGE: usize = 128 << 20;
const FLAG_NO_REPLY_EXPECTED: u8 = 0x1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    MethodCall = 1,
    MethodReturn = 2,
    Error = 3,
    Signal = 4,
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Marshals values in little-endian D-Bus format. Alignment is computed from
/// the start of the buffer, which matches the message because both headers and
/// bodies start on an 8-byte boundary.
#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn pad(&mut self, align: usize) {
        self.buf.resize(self.buf.len().next_multiple_of(align), 0);
    }

    pub fn byte(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.pad(4);
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.u32(v as u32)
    }

    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.u32(v.into())
    }

    /// A string or object path.
    pub fn str(&mut self, v: &str) -> &mut Self {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v.as_bytes());
        self.byte(0)
    }

    pub fn sig(&mut self, v: &str) -> &mut Self {
        self.byte(v.len() as u8);
        self.buf.extend_from_slice(v.as_bytes());
        self.byte(0)
    }

    /// A byte array (`ay`).
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.u32(v.len() as u32);
        self.buf.extend_from_slice(v);
        self
    }

    /// An array whose elements have alignment `align`, written by `f`.
    pub fn array(&mut self, align: usize, f: impl FnOnce(&mut Self)) -> &mut Self {
        self.u32(0);
        let len_at = self.buf.len() - 4;
        self.pad(align);
        let start = self.buf.len();
        f(self);
        let len = (self.buf.len() - start) as u32;
        self.buf[len_at..len_at + 4].copy_from_slice(&len.to_le_bytes());
        self
    }

    /// A struct or dict entry.
    pub fn structure(&mut self, f: impl FnOnce(&mut Self)) -> &mut Self {
        self.pad(8);
        f(self);
        self
    }

    pub fn variant(&mut self, sig: &str, f: impl FnOnce(&mut Self)) -> &mut Self {
        self.sig(sig);
        f(self);
        self
    }

    /// A `{sv}` dict entry.
    pub fn entry(&mut self, key: &str, sig: &str, f: impl FnOnce(&mut Self)) -> &mut Self {
        self.structure(|w| {
            w.str(key).variant(sig, f);
        })
    }
}

/// Unmarshals values of either byte order.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
    le: bool,
}

impl<'a> Reader<'a> {
    fn align(&mut self, align: usize) -> io::Result<()> {
        self.pos = self.pos.next_multiple_of(align);
        if self.pos > self.buf.len() {
            return Err(invalid("truncated message"));
        }
        Ok(())
    }

    fn take(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end =
            self.pos.checked_add(n).filter(|&end| end <= self.buf.len()).ok_or_else(|| invalid("truncated message"))?;
        let bytes = &self.buf[self.pos..end];
        self.pos = end;
        Ok(bytes)
    }

    pub fn byte(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub fn u32(&mut self) -> io::Result<u32> {
        self.align(4)?;
        let b: [u8; 4] = self.take(4)?.try_into().expect("4 bytes");
        Ok(if self.le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) })
    }

    pub fn i32(&mut self) -> io::Result<i32> {
        self.u32().map(|v| v as i32)
    }

    fn text(&mut self, len: usize) -> io::Result<&'a str> {
        let bytes = self.take(len + 1)?;
        if bytes[len] != 0 {
            return Err(invalid("unterminated string"));
        }
        str::from_utf8(&bytes[..len]).map_err(|_| invalid("invalid UTF-8"))
    }

    /// A string or object path.
    pub fn str(&mut self) -> io::Result<&'a str> {
        let len = self.u32()? as usize;
        self.text(len)
    }

    pub fn sig(&mut self) -> io::Result<&'a str> {
        let len = self.byte()? as usize;
        self.text(len)
    }

    /// Reads an array header and returns the offset where its elements end.
    pub fn array_end(&mut self, align: usize) -> io::Result<usize> {
        let len = self.u32()? as usize;
        self.align(align)?;
        let end = self.pos.checked_add(len).filter(|&end| end <= self.buf.len()).ok_or_else(|| invalid("bad array"))?;
        Ok(end)
    }

    pub fn before(&self, end: usize) -> bool {
        self.pos < end
    }

    pub fn struct_start(&mut self) -> io::Result<()> {
        self.align(8)
    }

    /// Skips over values of the given signature.
    pub fn skip(&mut self, sig: &str) -> io::Result<()> {
        let mut rest = sig.as_bytes();
        while !rest.is_empty() {
            let len = type_len(rest)?;
            self.skip_one(&rest[..len])?;
            rest = &rest[len..];
        }
        Ok(())
    }

    fn skip_one(&mut self, ty: &[u8]) -> io::Result<()> {
        match ty[0] {
            b'y' => self.take(1).map(drop),
            b'n' | b'q' => self.align(2).and_then(|()| self.take(2).map(drop)),
            b'b' | b'i' | b'u' | b'h' => self.u32().map(drop),
            b'x' | b't' | b'd' => self.align(8).and_then(|()| self.take(8).map(drop)),
            b's' | b'o' => self.str().map(drop),
            b'g' => self.sig().map(drop),
            b'v' => {
                let sig = self.sig()?;
                self.skip(sig)
            }
            b'a' => {
                let elem = &ty[1..];
                let end = self.array_end(alignment(elem[0]))?;
                while self.before(end) {
                    self.skip_one(elem)?;
                }
                if self.pos == end {
                    Ok(())
                } else {
                    Err(invalid("array overrun"))
                }
            }
            b'(' | b'{' => {
                self.align(8)?;
                self.skip(str::from_utf8(&ty[1..ty.len() - 1]).map_err(|_| invalid("bad signature"))?)
            }
            _ => Err(invalid("unsupported type")),
        }
    }
}

fn alignment(ty: u8) -> usize {
    match ty {
        b'y' | b'g' | b'v' => 1,
        b'n' | b'q' => 2,
        b'x' | b't' | b'd' | b'(' | b'{' => 8,
        _ => 4,
    }
}

/// Length of the first complete type in `sig`.
fn type_len(sig: &[u8]) -> io::Result<usize> {
    let close = match sig.first() {
        None => return Err(invalid("bad signature")),
        Some(b'a') => return Ok(1 + type_len(&sig[1..])?),
        Some(b'(') => b')',
        Some(b'{') => b'}',
        Some(_) => return Ok(1),
    };
    let mut i = 1;
    while sig.get(i) != Some(&close) {
        i += type_len(sig.get(i..).filter(|s| !s.is_empty()).ok_or_else(|| invalid("bad signature"))?)?;
    }
    Ok(i + 1)
}

/// A received message.
pub struct Message {
    pub kind: Kind,
    flags: u8,
    pub serial: u32,
    pub path: Option<String>,
    pub interface: Option<String>,
    pub member: Option<String>,
    pub error_name: Option<String>,
    pub reply_serial: Option<u32>,
    pub sender: Option<String>,
    pub signature: String,
    body: Vec<u8>,
    le: bool,
}

impl Message {
    pub fn body(&self) -> Reader<'_> {
        Reader { buf: &self.body, pos: 0, le: self.le }
    }

    pub fn member(&self) -> &str {
        self.member.as_deref().unwrap_or_default()
    }

    pub fn interface(&self) -> &str {
        self.interface.as_deref().unwrap_or_default()
    }

    /// The error name plus its message argument, if any.
    pub fn error_text(&self) -> String {
        let name = self.error_name.as_deref().unwrap_or("unknown error");
        match self.signature.starts_with('s').then(|| self.body().str().ok()).flatten() {
            Some(text) => format!("{name}: {text}"),
            None => name.to_owned(),
        }
    }

    fn parse(raw: Vec<u8>) -> io::Result<Self> {
        let le = raw[0] == b'l';
        let mut r = Reader { buf: &raw, pos: 1, le };
        let kind = match r.byte()? {
            1 => Kind::MethodCall,
            2 => Kind::MethodReturn,
            3 => Kind::Error,
            4 => Kind::Signal,
            _ => return Err(invalid("unknown message type")),
        };
        let flags = r.byte()?;
        r.byte()?; // protocol version
        let body_len = r.u32()? as usize;
        let serial = r.u32()?;

        let mut msg = Message {
            kind,
            flags,
            serial,
            path: None,
            interface: None,
            member: None,
            error_name: None,
            reply_serial: None,
            sender: None,
            signature: String::new(),
            body: Vec::new(),
            le,
        };
        let end = r.array_end(8)?;
        while r.before(end) {
            r.struct_start()?;
            let code = r.byte()?;
            let sig = r.sig()?;
            match (code, sig) {
                (1, "o") => msg.path = Some(r.str()?.into()),
                (2, "s") => msg.interface = Some(r.str()?.into()),
                (3, "s") => msg.member = Some(r.str()?.into()),
                (4, "s") => msg.error_name = Some(r.str()?.into()),
                (5, "u") => msg.reply_serial = Some(r.u32()?),
                (7, "s") => msg.sender = Some(r.str()?.into()),
                (8, "g") => msg.signature = r.sig()?.into(),
                _ => r.skip(sig)?,
            }
        }
        r.align(8)?;
        let body_start = r.pos;
        if raw.len() - body_start != body_len {
            return Err(invalid("bad body length"));
        }
        msg.body = raw[body_start..].to_vec();
        Ok(msg)
    }
}

/// Header fields of an outgoing message.
#[derive(Default)]
struct Header<'a> {
    flags: u8,
    path: Option<&'a str>,
    interface: Option<&'a str>,
    member: Option<&'a str>,
    error_name: Option<&'a str>,
    reply_serial: Option<u32>,
    destination: Option<&'a str>,
}

fn encode(kind: Kind, h: &Header, serial: u32, sig: &str, body: &[u8]) -> Vec<u8> {
    let mut w = Writer::default();
    w.byte(b'l').byte(kind as u8).byte(h.flags).byte(1).u32(body.len() as u32).u32(serial);
    w.array(8, |w| {
        let fields = [
            (1, "o", h.path),
            (2, "s", h.interface),
            (3, "s", h.member),
            (4, "s", h.error_name),
            (6, "s", h.destination),
        ];
        for (code, ty, value) in fields {
            if let Some(value) = value {
                w.structure(|w| {
                    w.byte(code).variant(ty, |w| {
                        w.str(value);
                    });
                });
            }
        }
        if let Some(serial) = h.reply_serial {
            w.structure(|w| {
                w.byte(5).variant("u", |w| {
                    w.u32(serial);
                });
            });
        }
        if !sig.is_empty() {
            w.structure(|w| {
                w.byte(8).variant("g", |w| {
                    w.sig(sig);
                });
            });
        }
    });
    w.pad(8);
    w.buf.extend_from_slice(body);
    w.buf
}

/// The sending half of a connection; shareable between threads.
pub struct Bus {
    stream: Mutex<UnixStream>,
    serial: AtomicU32,
}

/// The receiving half of a connection, owned by one thread.
pub struct Incoming {
    reader: BufReader<UnixStream>,
}

impl Incoming {
    pub fn next(&mut self) -> io::Result<Message> {
        let mut fixed = [0u8; 16];
        self.reader.read_exact(&mut fixed)?;
        let read = |at: usize| {
            let b = fixed[at..at + 4].try_into().expect("4 bytes");
            (if fixed[0] == b'l' { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }) as usize
        };
        let total = (16 + read(12)).next_multiple_of(8) + read(4);
        if total > MAX_MESSAGE {
            return Err(invalid("message too large"));
        }
        let mut raw = vec![0; total];
        raw[..16].copy_from_slice(&fixed);
        self.reader.read_exact(&mut raw[16..])?;
        Message::parse(raw)
    }
}

/// Connects and authenticates to the session bus.
pub fn session() -> io::Result<(Bus, Incoming)> {
    let mut stream = connect_session()?;
    authenticate(&mut stream)?;
    let bus = Bus { stream: Mutex::new(stream.try_clone()?), serial: AtomicU32::new(1) };
    let mut incoming = Incoming { reader: BufReader::new(stream) };

    let hello = bus.call(BUS_NAME, BUS_PATH, BUS_NAME, "Hello", "", |_| {})?;
    loop {
        let msg = incoming.next()?;
        if msg.reply_serial == Some(hello) {
            return match msg.kind {
                Kind::MethodReturn => Ok((bus, incoming)),
                _ => Err(io::Error::other(msg.error_text())),
            };
        }
    }
}

fn connect_session() -> io::Result<UnixStream> {
    let Ok(addresses) = env::var("DBUS_SESSION_BUS_ADDRESS") else {
        let runtime = env::var_os("XDG_RUNTIME_DIR").ok_or_else(|| io::Error::other("no session bus address"))?;
        return UnixStream::connect(std::path::Path::new(&runtime).join("bus"));
    };
    let mut last = io::Error::other("no usable session bus address");
    for address in addresses.split(';') {
        let Some(params) = address.strip_prefix("unix:") else { continue };
        for (key, value) in params.split(',').filter_map(|kv| kv.split_once('=')) {
            let value = unescape(value);
            let result = match key {
                "path" => UnixStream::connect(&value),
                #[cfg(any(target_os = "linux", target_os = "android"))]
                "abstract" => {
                    #[cfg(target_os = "android")]
                    use std::os::android::net::SocketAddrExt;
                    #[cfg(target_os = "linux")]
                    use std::os::linux::net::SocketAddrExt;
                    std::os::unix::net::SocketAddr::from_abstract_name(value.as_bytes())
                        .and_then(|addr| UnixStream::connect_addr(&addr))
                }
                _ => continue,
            };
            match result {
                Ok(stream) => return Ok(stream),
                Err(e) => last = e,
            }
        }
    }
    Err(last)
}

/// Decodes `%xx` escapes in an address value.
fn unescape(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| bytes.get(i + 1..i + 3)).flatten();
        match hex.and_then(|h| u8::from_str_radix(str::from_utf8(h).ok()?, 16).ok()) {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn authenticate(stream: &mut UnixStream) -> io::Result<()> {
    unsafe extern "C" {
        safe fn geteuid() -> u32;
    }
    let uid: String = geteuid().to_string().bytes().map(|b| format!("{b:02x}")).collect();
    stream.write_all(format!("\0AUTH EXTERNAL {uid}\r\n").as_bytes())?;
    // Read byte-wise so nothing past the reply line is consumed.
    let mut line = Vec::new();
    let mut byte = [0u8];
    while !line.ends_with(b"\r\n") {
        stream.read_exact(&mut byte)?;
        line.push(byte[0]);
        if line.len() > 512 {
            return Err(invalid("bad auth reply"));
        }
    }
    if !line.starts_with(b"OK ") {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "D-Bus authentication rejected"));
    }
    stream.write_all(b"BEGIN\r\n")
}

impl Bus {
    /// Sends a method call and returns its serial, to match against the reply.
    pub fn call(
        &self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        sig: &str,
        body: impl FnOnce(&mut Writer),
    ) -> io::Result<u32> {
        let header = Header {
            path: Some(path),
            interface: Some(interface),
            member: Some(member),
            destination: Some(destination),
            ..Header::default()
        };
        self.send(Kind::MethodCall, &header, sig, body)
    }

    pub fn signal(
        &self,
        path: &str,
        interface: &str,
        member: &str,
        sig: &str,
        body: impl FnOnce(&mut Writer),
    ) -> io::Result<()> {
        let header = Header { path: Some(path), interface: Some(interface), member: Some(member), ..Header::default() };
        self.send(Kind::Signal, &header, sig, body).map(drop)
    }

    pub fn reply(&self, to: &Message, sig: &str, body: impl FnOnce(&mut Writer)) -> io::Result<()> {
        if to.flags & FLAG_NO_REPLY_EXPECTED != 0 {
            return Ok(());
        }
        let header = Header {
            flags: FLAG_NO_REPLY_EXPECTED,
            reply_serial: Some(to.serial),
            destination: to.sender.as_deref(),
            ..Header::default()
        };
        self.send(Kind::MethodReturn, &header, sig, body).map(drop)
    }

    pub fn error(&self, to: &Message, name: &str, text: &str) -> io::Result<()> {
        if to.flags & FLAG_NO_REPLY_EXPECTED != 0 {
            return Ok(());
        }
        let header = Header {
            flags: FLAG_NO_REPLY_EXPECTED,
            error_name: Some(name),
            reply_serial: Some(to.serial),
            destination: to.sender.as_deref(),
            ..Header::default()
        };
        self.send(Kind::Error, &header, "s", |w| {
            w.str(text);
        })
        .map(drop)
    }

    fn send(&self, kind: Kind, header: &Header, sig: &str, body: impl FnOnce(&mut Writer)) -> io::Result<u32> {
        let mut w = Writer::default();
        body(&mut w);
        let serial = self.serial.fetch_add(1, Ordering::Relaxed);
        let msg = encode(kind, header, serial, sig, &w.buf);
        self.stream.lock().unwrap_or_else(PoisonError::into_inner).write_all(&msg)?;
        Ok(serial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(sig: &str, body: impl FnOnce(&mut Writer)) -> Message {
        let mut w = Writer::default();
        body(&mut w);
        let header = Header { path: Some("/a"), member: Some("M"), ..Header::default() };
        Message::parse(encode(Kind::Signal, &header, 7, sig, &w.buf)).unwrap()
    }

    #[test]
    fn header_roundtrip() {
        let msg = roundtrip("su", |w| {
            w.str("hi").u32(5);
        });
        assert_eq!((msg.kind, msg.serial, msg.path.as_deref(), msg.member()), (Kind::Signal, 7, Some("/a"), "M"));
        assert_eq!(msg.signature, "su");
        let mut r = msg.body();
        assert_eq!((r.str().unwrap(), r.u32().unwrap()), ("hi", 5));
    }

    #[test]
    fn nested_values_align_and_skip() {
        // a{sv} holding a struct, then a trailing int: exercises padding inside arrays and variants.
        let msg = roundtrip("a{sv}yi", |w| {
            w.array(8, |w| {
                w.entry("img", "(iiay)", |w| {
                    w.structure(|w| {
                        w.i32(2).i32(1).bytes(&[1, 2, 3]);
                    });
                });
                w.entry("name", "s", |w| {
                    w.str("x");
                });
            });
            w.byte(9).i32(-4);
        });
        let mut r = msg.body();
        r.skip("a{sv}y").unwrap();
        assert_eq!(r.i32().unwrap(), -4);

        let mut r = msg.body();
        let end = r.array_end(8).unwrap();
        r.struct_start().unwrap();
        assert_eq!(r.str().unwrap(), "img");
        assert_eq!(r.sig().unwrap(), "(iiay)");
        r.struct_start().unwrap();
        assert_eq!((r.i32().unwrap(), r.i32().unwrap()), (2, 1));
        assert!(r.before(end));
    }

    #[test]
    fn type_lengths() {
        assert_eq!(type_len(b"a{sv}i").unwrap(), 5);
        assert_eq!(type_len(b"(ia{sv}av)").unwrap(), 10);
        assert_eq!(type_len(b"aai").unwrap(), 3);
        assert!(type_len(b"(ii").is_err());
        assert!(type_len(b"a").is_err());
    }

    #[test]
    fn rejects_truncated_and_bad_strings() {
        let mut w = Writer::default();
        w.str("abc");
        w.buf.truncate(6);
        assert!(Reader { buf: &w.buf, pos: 0, le: true }.str().is_err());
        let bad = [3, 0, 0, 0, b'a', b'b', b'c', b'!'];
        assert!(Reader { buf: &bad, pos: 0, le: true }.str().is_err());
    }

    #[test]
    fn unescapes_addresses() {
        assert_eq!(unescape("/run/user/1000/bus"), "/run/user/1000/bus");
        assert_eq!(unescape("/tmp/a%20b%2c"), "/tmp/a b,");
        assert_eq!(unescape("50%"), "50%");
    }
}
