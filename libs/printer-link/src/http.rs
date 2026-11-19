/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! The one HTTP request a printer upload needs: a multipart POST, plain
//! HTTP/1.1 over a TCP stream, read to the end. A printer's web server is
//! small; this asks it for nothing more.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

/// A text field of a multipart form.
#[derive(Debug)]
pub struct Field<'a> {
    pub name: &'a str,
    pub value: &'a str,
}

/// The file of a multipart form.
#[derive(Debug)]
pub struct FilePart<'a> {
    pub field: &'a str,
    pub file_name: &'a str,
    pub data: &'a [u8],
}

/// What the server answered: its status code and its body.
#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

/// The multipart body of `fields` and `file`, between `boundary` lines.
pub fn multipart_body(boundary: &str, fields: &[Field<'_>], file: &FilePart<'_>) -> Vec<u8> {
    let mut body = Vec::with_capacity(file.data.len() + 512);
    for field in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n{}\r\n",
                field.name, field.value
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n",
            file.field,
            file.file_name.replace(['"', '\r', '\n'], "_")
        )
        .as_bytes(),
    );
    body.extend_from_slice(file.data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// POSTs a multipart form of `fields` and `file` to `path` on `addr`, and
/// reads the answer. `timeout` bounds the connection and each read and write.
pub fn post_multipart(
    addr: SocketAddr,
    path: &str,
    fields: &[Field<'_>],
    file: &FilePart<'_>,
    timeout: Duration,
) -> io::Result<Response> {
    let boundary = format!("----breadslicer{:016x}", crate::sdcp::random_u64());
    let body = multipart_body(&boundary, fields, file);
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nUser-Agent: Breadslicer\r\nAccept: */*\r\n\
         Content-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let mut request = head.into_bytes();
    request.extend_from_slice(&body);
    stream.write_all(&request)?;
    stream.flush()?;
    // Read until the answer is whole, not until the server closes: a
    // printer that keeps the connection open would hold every chunk for the
    // whole timeout.
    let mut raw = Vec::new();
    let mut buffer = [0u8; 8192];
    loop {
        if answer_is_whole(&raw) {
            break;
        }
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buffer[..n]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    parse_response(&raw)
}

/// Whether `raw` holds a whole HTTP answer: its headers and the body they
/// announce, by its length or its last chunk. An answer that announces
/// neither ends when the server closes.
pub fn answer_is_whole(raw: &[u8]) -> bool {
    let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let body = &raw[split + 4..];
    let length = head.lines().find_map(|line| {
        line.strip_prefix("content-length:")
            .and_then(|n| n.trim().parse::<usize>().ok())
    });
    if let Some(length) = length {
        return body.len() >= length;
    }
    let chunked = head
        .lines()
        .any(|line| line.starts_with("transfer-encoding:") && line.contains("chunked"));
    if chunked {
        return body.starts_with(b"0\r\n") || body.windows(5).any(|w| w == b"\r\n0\r\n");
    }
    // No length: a status that has no body is whole with its headers.
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok());
    matches!(status, Some(100..=199 | 204 | 304))
}

/// The status and body of a raw HTTP/1.1 answer, its body de-chunked when it
/// came in chunks.
pub fn parse_response(raw: &[u8]) -> io::Result<Response> {
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| bad("the printer's answer has no end of headers"))?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| bad("the printer's answer has no status"))?;
    let chunked = lines.any(|line| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("transfer-encoding:") && lower.contains("chunked")
    });
    let rest = &raw[split + 4..];
    let body = if chunked {
        dechunk(rest)
    } else {
        rest.to_vec()
    };
    Ok(Response { status, body })
}

/// A chunked body put back together; what does not parse ends it.
fn dechunk(mut rest: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    loop {
        let Some(eol) = rest.windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let size = std::str::from_utf8(&rest[..eol])
            .ok()
            .and_then(|s| usize::from_str_radix(s.split(';').next().unwrap_or("").trim(), 16).ok());
        let Some(size) = size else {
            break;
        };
        rest = &rest[eol + 2..];
        if size == 0 || rest.len() < size {
            body.extend_from_slice(&rest[..size.min(rest.len())]);
            break;
        }
        body.extend_from_slice(&rest[..size]);
        rest = rest.get(size + 2..).unwrap_or(&[]);
    }
    body
}
