/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! SDCP v3, how Elegoo's network printers take a print.
//!
//! - **Find**: `M99999` sent over UDP to port 3000, broadcast on every
//!   network interface or to one address; each printer answers with a JSON
//!   description of itself (name, model, IP, mainboard id).
//! - **Send**: the printer file in 1 MiB chunks, each an HTTP multipart POST
//!   to `/uploadFile/upload` on port 3030, carrying the upload's id, the
//!   chunk's offset and the file's total size.
//! - **Print** and **status**: JSON commands over a WebSocket on
//!   `/websocket`, addressed to the printer's mainboard id; command 128
//!   starts a print of a file on the printer, command 0 asks its status.

use std::collections::HashSet;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tungstenite::Message;

use md5::Digest;

use crate::http::{Field, FilePart, post_multipart};

/// The UDP port a printer listens for `M99999` on.
pub const DISCOVERY_PORT: u16 = 3000;
/// The port of a printer's web server and WebSocket.
pub const SERVICE_PORT: u16 = 3030;
/// How much of the file each upload request carries.
pub const CHUNK_BYTES: usize = 1024 * 1024;
/// How long one chunk may take to go.
const CHUNK_TIMEOUT: Duration = Duration::from_secs(20);
/// How long a command waits for the printer's answer.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(6);

/// A printer on the network, as it described itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Printer {
    pub ip: Ipv4Addr,
    /// What every command is addressed to.
    pub mainboard_id: String,
    /// The name the printer was given, or its model's.
    pub name: String,
    /// Its model, as it names it.
    pub model: String,
    pub firmware: String,
}

impl Printer {
    /// Its web server and WebSocket.
    pub fn service(&self) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(self.ip, SERVICE_PORT))
    }
}

/// Why sending or printing failed.
#[derive(Debug)]
pub enum Error {
    /// The printer could not be reached or stopped answering.
    Unreachable(io::Error),
    /// Its web server refused the upload: the status and what it said.
    Upload(u16, String),
    /// It answered something this does not read.
    Protocol(String),
    /// It refused to start the print.
    Refused(Refusal),
    /// No answer in time.
    Timeout,
    /// Stopped on the way.
    Cancelled,
}

/// Why a printer refused to start a print (the command's `Ack`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// It is printing or busy.
    Busy,
    /// The file is not on it.
    NotFound,
    /// The file came damaged.
    Damaged,
    /// It could not read the file.
    Unreadable,
    /// The file is for another screen.
    Resolution,
    /// It does not know the file's format.
    Format,
    /// The file is for another printer model.
    Model,
    /// A code this does not know.
    Other(i64),
}

impl Refusal {
    pub fn from_ack(ack: i64) -> Option<Self> {
        Some(match ack {
            0 => return None,
            1 => Self::Busy,
            2 => Self::NotFound,
            3 => Self::Damaged,
            4 => Self::Unreadable,
            5 => Self::Resolution,
            6 => Self::Format,
            7 => Self::Model,
            other => Self::Other(other),
        })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(err) => write!(f, "the printer could not be reached: {err}"),
            Self::Upload(status, said) => {
                write!(f, "the printer refused the file (HTTP {status}): {said}")
            }
            Self::Protocol(what) => write!(f, "the printer answered something unexpected: {what}"),
            Self::Refused(why) => write!(f, "the printer refused to start: {why:?}"),
            Self::Timeout => f.write_str("the printer did not answer in time"),
            Self::Cancelled => f.write_str("stopped"),
        }
    }
}

impl std::error::Error for Error {}

/// What a printer is doing, as its status says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Machine {
    Idle,
    Printing,
    /// Taking a file.
    Receiving,
    /// An exposure test.
    Testing,
    /// Checking itself.
    SelfCheck,
    Other(i64),
}

/// A printer's status: what it does, and the print on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub machine: Machine,
    /// The layer printed and the layers, while it prints.
    pub layers: Option<(u32, u32)>,
    /// The file it prints or printed last.
    pub file: Option<String>,
}

/// A random 64-bit number: an upload's id, a form's boundary.
pub(crate) fn random_u64() -> u64 {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    hasher.write_u32(std::process::id());
    hasher.finish()
}

/// A fresh upload id, written as a UUID (version 4) is.
pub fn upload_id() -> String {
    let (a, b) = (random_u64(), random_u64());
    let hi = (a & !0xF000) | 0x4000;
    let lo = (b & !(0b11 << 62)) | (0b10 << 62);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        hi >> 32,
        (hi >> 16) & 0xFFFF,
        hi & 0xFFFF,
        lo >> 48,
        lo & 0xFFFF_FFFF_FFFF
    )
}

/// The name a file goes onto the printer by: what its firmware takes,
/// letters, digits, dot, dash and underscore, its extension kept.
pub fn printer_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() || trimmed.starts_with('.') {
        format!("print{trimmed}")
    } else {
        trimmed.to_owned()
    }
}

/// Where `M99999` goes: every interface's broadcast address, and the
/// limited broadcast.
fn broadcast_targets() -> Vec<Ipv4Addr> {
    let mut targets = vec![Ipv4Addr::BROADCAST];
    if let Ok(interfaces) = if_addrs::get_if_addrs() {
        for interface in interfaces {
            if interface.is_loopback() {
                continue;
            }
            if let if_addrs::IfAddr::V4(v4) = interface.addr {
                let ip = u32::from(v4.ip);
                let mask = u32::from(v4.netmask);
                let broadcast = Ipv4Addr::from(ip | !mask);
                if !targets.contains(&broadcast) {
                    targets.push(broadcast);
                }
            }
        }
    }
    targets
}

/// The printers that answer on the local network within `wait`.
pub fn discover(wait: Duration) -> Vec<Printer> {
    ask_targets(&broadcast_targets(), DISCOVERY_PORT, wait, None)
}

/// The printer at `ip`, if it answers within `wait`: for an address typed by
/// hand.
pub fn ask(ip: Ipv4Addr, wait: Duration) -> Option<Printer> {
    ask_at(ip, DISCOVERY_PORT, wait)
}

/// [`ask`] on another port: for tests.
pub fn ask_at(ip: Ipv4Addr, port: u16, wait: Duration) -> Option<Printer> {
    ask_targets(&[ip], port, wait, Some(1)).into_iter().next()
}

/// Sends `M99999` to `targets` on `port` and gathers the answers for
/// `wait`, or until `enough` came.
fn ask_targets(
    targets: &[Ipv4Addr],
    port: u16,
    wait: Duration,
    enough: Option<usize>,
) -> Vec<Printer> {
    let Ok(socket) = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0)) else {
        return Vec::new();
    };
    let _ = socket.set_broadcast(true);
    let _ = socket.set_read_timeout(Some(Duration::from_millis(100)));
    for target in targets {
        let _ = socket.send_to(b"M99999", SocketAddrV4::new(*target, port));
    }
    let (start, mut seen, mut found) = (Instant::now(), HashSet::new(), Vec::new());
    let mut buffer = [0u8; 4096];
    while start.elapsed() < wait {
        let Ok((size, from)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        let IpAddr::V4(from) = from.ip() else {
            continue;
        };
        let Some(printer) = parse_discovery(&buffer[..size], from) else {
            continue;
        };
        if seen.insert(printer.ip) {
            found.push(printer);
            if enough.is_some_and(|n| found.len() >= n) {
                break;
            }
        }
    }
    found
}

/// A printer's answer to `M99999`, sent from `from`. Its fields sit in
/// `Data`, or at the top in older firmware.
pub fn parse_discovery(raw: &[u8], from: Ipv4Addr) -> Option<Printer> {
    let value: Value = serde_json::from_slice(raw).ok()?;
    let data = value
        .get("Data")
        .filter(|d| d.is_object())
        .unwrap_or(&value);
    let text = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| data.get(*k).and_then(Value::as_str))
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    let mainboard_id = text(&["MainboardID"])?;
    let ip = text(&["MainboardIP"])
        .and_then(|ip| ip.parse().ok())
        .unwrap_or(from);
    let model = text(&["MachineName", "Model"]).unwrap_or_default();
    Some(Printer {
        ip,
        name: text(&["Name"]).unwrap_or_else(|| model.clone()),
        model,
        firmware: text(&["FirmwareVersion", "Version"]).unwrap_or_default(),
        mainboard_id,
    })
}

/// How many chunks go at once. A printer's network stack takes little at a
/// time on one connection; several connections share the Wi-Fi better.
pub const CONNECTIONS: usize = 4;

/// How a file went to the printer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sent {
    /// The connections it went over at once.
    pub connections: usize,
    pub elapsed: Duration,
    /// Of all chunks, the time spent sending and the time waiting for the
    /// printer's answer.
    pub writing: Duration,
    pub waiting: Duration,
    /// Why it was sent again one chunk at a time, when it was.
    pub fell_back: Option<String>,
}

/// Sends `data` to the printer at `service` as `file_name`, telling
/// `progress` the bytes sent and the total as it goes; `cancel` stops it
/// between chunks.
///
/// First over [`CONNECTIONS`] connections at once, with the file's MD5 for
/// the printer to check it by, the last chunk going alone after all the
/// others. Should the printer refuse that, the file goes again from the
/// start a chunk at a time, unchecked, as every SDCP printer takes it.
pub fn upload(
    service: SocketAddr,
    file_name: &str,
    data: &[u8],
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<Sent, Error> {
    let start = Instant::now();
    match upload_with(
        service,
        file_name,
        data,
        CONNECTIONS,
        true,
        progress,
        cancel,
    ) {
        Ok(mut sent) => {
            sent.elapsed = start.elapsed();
            Ok(sent)
        }
        Err(Error::Cancelled) => Err(Error::Cancelled),
        Err(first) => {
            let mut sent = upload_with(service, file_name, data, 1, false, progress, cancel)?;
            sent.elapsed = start.elapsed();
            sent.fell_back = Some(first.to_string());
            Ok(sent)
        }
    }
}

/// One chunk of an upload: where it goes in the file, and its bytes.
struct Chunk<'a> {
    offset: usize,
    data: &'a [u8],
}

/// What an upload says of the whole file with every chunk.
struct Upload<'a> {
    service: SocketAddr,
    file_name: &'a str,
    id: String,
    total: String,
    md5: String,
    check: &'static str,
}

impl Upload<'_> {
    /// Sends `chunk`; the time it took to go and to be answered.
    fn send(&self, chunk: &Chunk<'_>) -> Result<(Duration, Duration), Error> {
        let offset = chunk.offset.to_string();
        let fields = [
            Field {
                name: "S-File-MD5",
                value: &self.md5,
            },
            Field {
                name: "Check",
                value: self.check,
            },
            Field {
                name: "Offset",
                value: &offset,
            },
            Field {
                name: "Uuid",
                value: &self.id,
            },
            Field {
                name: "TotalSize",
                value: &self.total,
            },
        ];
        let file = FilePart {
            field: "File",
            file_name: self.file_name,
            data: chunk.data,
        };
        let answer = post_multipart(
            self.service,
            "/uploadFile/upload",
            &fields,
            &file,
            CHUNK_TIMEOUT,
        )
        .map_err(Error::Unreachable)?;
        let said = String::from_utf8_lossy(&answer.body).into_owned();
        if !(200..300).contains(&answer.status) {
            return Err(Error::Upload(answer.status, said));
        }
        // Its body says whether it took the chunk, when it says anything.
        if let Ok(body) = serde_json::from_str::<Value>(&said)
            && body.get("success").and_then(Value::as_bool) == Some(false)
        {
            return Err(Error::Upload(answer.status, said));
        }
        Ok((answer.writing, answer.waiting))
    }
}

/// [`upload`] over `connections` at once, `checked` by the file's MD5.
fn upload_with(
    service: SocketAddr,
    file_name: &str,
    data: &[u8],
    connections: usize,
    checked: bool,
    progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<Sent, Error> {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, AtomicUsize};

    let upload = Upload {
        service,
        file_name,
        id: upload_id(),
        total: data.len().to_string(),
        md5: if checked {
            format!("{:x}", md5::Md5::digest(data))
        } else {
            String::new()
        },
        check: if checked { "1" } else { "0" },
    };
    let chunks: Vec<Chunk<'_>> = (0..data.len().div_ceil(CHUNK_BYTES).max(1))
        .map(|k| {
            let offset = k * CHUNK_BYTES;
            Chunk {
                offset,
                data: &data[offset..(offset + CHUNK_BYTES).min(data.len())],
            }
        })
        .collect();
    let total = data.len() as u64;
    let start = Instant::now();
    progress(0, total);
    // All chunks but the last at once; the last after them, alone, so the
    // file is whole when the printer sees its end.
    let (last, rest) = chunks.split_last().expect("one chunk at least");
    let sent = AtomicU64::new(0);
    let next = AtomicUsize::new(0);
    let failed: Mutex<Option<Error>> = Mutex::new(None);
    let times = Mutex::new((Duration::ZERO, Duration::ZERO));
    let stop = AtomicBool::new(false);
    let work = || {
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            if cancel.load(Ordering::Relaxed) {
                stop.store(true, Ordering::Relaxed);
                let mut failed = failed.lock().expect("not poisoned");
                failed.get_or_insert(Error::Cancelled);
                return;
            }
            let k = next.fetch_add(1, Ordering::Relaxed);
            let Some(chunk) = rest.get(k) else {
                return;
            };
            match upload.send(chunk) {
                Ok((writing, waiting)) => {
                    let mut times = times.lock().expect("not poisoned");
                    times.0 += writing;
                    times.1 += waiting;
                    sent.fetch_add(chunk.data.len() as u64, Ordering::Relaxed);
                }
                Err(err) => {
                    stop.store(true, Ordering::Relaxed);
                    failed.lock().expect("not poisoned").get_or_insert(err);
                    return;
                }
            }
        }
    };
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..connections.clamp(1, rest.len().max(1)))
            .map(|_| scope.spawn(work))
            .collect();
        while !workers.iter().all(|w| w.is_finished()) {
            progress(sent.load(Ordering::Relaxed), total);
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    if let Some(err) = failed.into_inner().expect("not poisoned") {
        return Err(err);
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    let (writing, waiting) = upload.send(last)?;
    let (mut all_writing, mut all_waiting) = times.into_inner().expect("not poisoned");
    all_writing += writing;
    all_waiting += waiting;
    progress(total, total);
    Ok(Sent {
        connections: connections.clamp(1, chunks.len()),
        elapsed: start.elapsed(),
        writing: all_writing,
        waiting: all_waiting,
        fell_back: None,
    })
}

/// A command's request, as the printer reads it.
fn request(mainboard_id: &str, cmd: u32, data: Value, request_id: &str) -> Value {
    json!({
        "Id": "breadslicer",
        "Data": {
            "Cmd": cmd,
            "Data": data,
            "RequestID": request_id,
            "MainboardID": mainboard_id,
            "TimeStamp": SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            "From": 0,
        },
        "Topic": format!("sdcp/request/{mainboard_id}"),
    })
}

/// A WebSocket to the printer at `service`, reads bounded by `timeout`.
fn connect(
    service: SocketAddr,
    timeout: Duration,
) -> Result<tungstenite::WebSocket<TcpStream>, Error> {
    let stream = TcpStream::connect_timeout(&service, timeout).map_err(Error::Unreachable)?;
    stream
        .set_read_timeout(Some(Duration::from_millis(250)))
        .map_err(Error::Unreachable)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(Error::Unreachable)?;
    let url = format!("ws://{service}/websocket");
    let (socket, _) = tungstenite::client(url.as_str(), stream).map_err(|err| match err {
        tungstenite::HandshakeError::Failure(err) => Error::Protocol(err.to_string()),
        tungstenite::HandshakeError::Interrupted(_) => Error::Timeout,
    })?;
    Ok(socket)
}

/// Sends command `cmd` with `data` to the printer, and reads frames until
/// `wanted` takes one or `timeout` passes. Every frame read goes to `wanted`.
fn command<T>(
    service: SocketAddr,
    mainboard_id: &str,
    cmd: u32,
    data: Value,
    timeout: Duration,
    mut wanted: impl FnMut(&Value, &str) -> Option<T>,
) -> Result<T, Error> {
    let mut socket = connect(service, timeout)?;
    let request_id = format!("{:016x}{:016x}", random_u64(), random_u64());
    socket
        .send(Message::Text(
            request(mainboard_id, cmd, data, &request_id)
                .to_string()
                .into(),
        ))
        .map_err(|err| Error::Protocol(err.to_string()))?;
    let start = Instant::now();
    while start.elapsed() < timeout {
        let text = match socket.read() {
            Ok(Message::Text(text)) => text.to_string(),
            Ok(Message::Binary(data)) => String::from_utf8_lossy(&data).into_owned(),
            Ok(Message::Close(_)) => break,
            Ok(_) => continue,
            Err(tungstenite::Error::Io(err))
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(err) => return Err(Error::Protocol(err.to_string())),
        };
        let Ok(frame) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Some(found) = wanted(&frame, &request_id) {
            let _ = socket.close(None);
            return Ok(found);
        }
    }
    let _ = socket.close(None);
    Err(Error::Timeout)
}

/// The `Ack` of the answer to request `request_id`, when `frame` is it.
pub fn ack_of(frame: &Value, request_id: &str) -> Option<i64> {
    let data = frame.get("Data")?;
    if data.get("RequestID").and_then(Value::as_str) != Some(request_id) {
        return None;
    }
    Some(
        data.pointer("/Data/Ack")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    )
}

/// Starts printing `file_name`, a file on the printer, from its first layer.
pub fn start_print(service: SocketAddr, mainboard_id: &str, file_name: &str) -> Result<(), Error> {
    let ack = command(
        service,
        mainboard_id,
        128,
        json!({ "Filename": file_name, "StartLayer": 0 }),
        COMMAND_TIMEOUT,
        ack_of,
    )?;
    match Refusal::from_ack(ack) {
        None => Ok(()),
        Some(why) => Err(Error::Refused(why)),
    }
}

/// The printer's status. It sends one on its own topic as it is asked.
pub fn status(service: SocketAddr, mainboard_id: &str) -> Result<Status, Error> {
    command(
        service,
        mainboard_id,
        0,
        json!({}),
        COMMAND_TIMEOUT,
        |frame, _| status_of(frame),
    )
}

/// The status a status frame carries.
pub fn status_of(frame: &Value) -> Option<Status> {
    let status = frame
        .get("Status")
        .or_else(|| frame.pointer("/Data/Status"))?;
    let current = status.get("CurrentStatus")?;
    let code = current
        .as_array()
        .and_then(|codes| codes.first())
        .unwrap_or(current)
        .as_i64()?;
    let machine = match code {
        0 => Machine::Idle,
        1 => Machine::Printing,
        2 => Machine::Receiving,
        3 => Machine::Testing,
        4 => Machine::SelfCheck,
        other => Machine::Other(other),
    };
    let info = status.get("PrintInfo");
    let number = |key: &str| {
        info.and_then(|i| i.get(key))
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    };
    let layers = match (number("CurrentLayer"), number("TotalLayer")) {
        (Some(at), Some(of)) if of > 0 => Some((at, of)),
        _ => None,
    };
    let file = info
        .and_then(|i| i.get("Filename"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .filter(|f| !f.is_empty());
    Some(Status {
        machine,
        layers,
        file,
    })
}
