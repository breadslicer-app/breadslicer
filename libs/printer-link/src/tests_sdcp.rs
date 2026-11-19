/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! SDCP against a printer played on this machine: its UDP answer, its web
//! server taking chunks, its WebSocket answering commands.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::Message;

use crate::http::{FilePart, multipart_body, parse_response};
use crate::sdcp::*;

const ANSWER: &str = r#"{"Id":"x","Data":{"Name":"Saturno","MachineName":"ELEGOO Saturn 4 Ultra","BrandName":"ELEGOO","MainboardIP":"127.0.0.1","MainboardID":"abc123def456","ProtocolVersion":"V3.0.0","FirmwareVersion":"V1.2.3"}}"#;

#[test]
fn a_printer_answers_m99999_with_who_it_is() {
    let printer = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = printer.local_addr().unwrap().port();
    thread::spawn(move || {
        let mut buffer = [0u8; 64];
        let (size, from) = printer.recv_from(&mut buffer).unwrap();
        assert_eq!(&buffer[..size], b"M99999");
        printer.send_to(ANSWER.as_bytes(), from).unwrap();
    });
    let found = ask_at(Ipv4Addr::LOCALHOST, port, Duration::from_secs(3)).expect("it answers");
    assert_eq!(
        found,
        Printer {
            ip: Ipv4Addr::LOCALHOST,
            mainboard_id: "abc123def456".into(),
            name: "Saturno".into(),
            model: "ELEGOO Saturn 4 Ultra".into(),
            firmware: "V1.2.3".into(),
        }
    );
}

#[test]
fn an_older_answer_with_its_fields_on_top_still_reads() {
    let raw = br#"{"MainboardID":"m1","Name":"","Model":"Mars 4 Ultra"}"#;
    let printer = parse_discovery(raw, Ipv4Addr::new(10, 0, 0, 7)).unwrap();
    assert_eq!(
        printer.ip,
        Ipv4Addr::new(10, 0, 0, 7),
        "the sender's address"
    );
    assert_eq!(printer.name, "Mars 4 Ultra", "no name: its model's");
    assert!(parse_discovery(b"not json", Ipv4Addr::LOCALHOST).is_none());
    assert!(parse_discovery(br#"{"Data":{}}"#, Ipv4Addr::LOCALHOST).is_none());
}

/// One upload request as the printer's server read it.
#[derive(Debug, Clone)]
struct Chunk {
    fields: Vec<(String, String)>,
    file_name: String,
    data: Vec<u8>,
}

/// The parts of a multipart body.
fn parts(body: &[u8], boundary: &str) -> Chunk {
    let marker = format!("--{boundary}");
    let text = body;
    let mut chunk = Chunk {
        fields: Vec::new(),
        file_name: String::new(),
        data: Vec::new(),
    };
    let mut at = 0;
    let find = |hay: &[u8], needle: &[u8], from: usize| {
        hay[from..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|p| p + from)
    };
    while let Some(start) = find(text, marker.as_bytes(), at) {
        let after = start + marker.len();
        if text[after..].starts_with(b"--") {
            break;
        }
        let head_end = find(text, b"\r\n\r\n", after).unwrap();
        let head = String::from_utf8_lossy(&text[after..head_end]).into_owned();
        let next = find(text, format!("\r\n{marker}").as_bytes(), head_end).unwrap();
        let value = &text[head_end + 4..next];
        let name = head
            .split("name=\"")
            .nth(1)
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        if let Some(file) = head.split("filename=\"").nth(1) {
            chunk.file_name = file.split('"').next().unwrap().to_owned();
            chunk.data = value.to_vec();
        } else {
            chunk
                .fields
                .push((name.to_owned(), String::from_utf8_lossy(value).into_owned()));
        }
        at = next + 2;
    }
    chunk
}

const OK: &str = r#"{"code":"000000","messages":null,"data":null,"success":true}"#;

/// A printer's web server: each request answered by `answer`, each
/// connection on a thread of its own and, like a printer's, kept open after
/// its answer. What it took, in the order the chunks came.
fn web_server(
    answer: impl Fn(&Chunk) -> (u16, String) + Send + Sync + 'static,
) -> (SocketAddr, Arc<Mutex<Vec<Chunk>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let got = Arc::new(Mutex::new(Vec::new()));
    let seen = got.clone();
    let answer = Arc::new(answer);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            let (seen, answer) = (seen.clone(), answer.clone());
            thread::spawn(move || {
                let mut raw = Vec::new();
                let mut buffer = [0u8; 65536];
                let (head_end, length, boundary) = loop {
                    let n = stream.read(&mut buffer).unwrap();
                    raw.extend_from_slice(&buffer[..n]);
                    if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&raw[..end]).into_owned();
                        let length: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .unwrap()
                            .trim()
                            .parse()
                            .unwrap();
                        let boundary = head
                            .split("boundary=")
                            .nth(1)
                            .unwrap()
                            .lines()
                            .next()
                            .unwrap()
                            .to_owned();
                        break (end + 4, length, boundary);
                    }
                };
                while raw.len() < head_end + length {
                    let n = stream.read(&mut buffer).unwrap();
                    raw.extend_from_slice(&buffer[..n]);
                }
                let chunk = parts(&raw[head_end..], &boundary);
                let (status, body) = answer(&chunk);
                seen.lock().unwrap().push(chunk);
                let reply = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(reply.as_bytes()).unwrap();
                thread::sleep(Duration::from_secs(60));
            });
        }
    });
    (addr, got)
}

fn field(c: &Chunk, name: &str) -> String {
    c.fields
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
        .unwrap()
}

#[test]
fn a_file_goes_in_chunks_at_once_that_add_up_to_it_and_its_md5() {
    use md5::Digest;
    let data: Vec<u8> = (0..(CHUNK_BYTES * 5 + 1234))
        .map(|i| (i % 251) as u8)
        .collect();
    let (addr, got) = web_server(|_| (200, OK.to_owned()));
    let mut seen = Vec::new();
    let sent = upload(
        addr,
        "plate.goo",
        &data,
        &mut |at, of| seen.push((at, of)),
        &AtomicBool::new(false),
    )
    .expect("it takes the file");
    assert_eq!(sent.connections, CONNECTIONS);
    assert_eq!(sent.fell_back, None);
    let total = data.len() as u64;
    assert_eq!(seen.first(), Some(&(0, total)));
    assert_eq!(seen.last(), Some(&(total, total)));
    assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0), "never back");
    let chunks = got.lock().unwrap().clone();
    assert_eq!(chunks.len(), 6);
    // The last chunk comes after all the others.
    let last_offset = (5 * CHUNK_BYTES).to_string();
    assert_eq!(field(chunks.last().unwrap(), "Offset"), last_offset);
    let md5 = format!("{:x}", md5::Md5::digest(&data));
    let id = field(&chunks[0], "Uuid");
    let mut by_offset = chunks.clone();
    by_offset.sort_by_key(|c| field(c, "Offset").parse::<usize>().unwrap());
    let mut joined = Vec::new();
    for (k, chunk) in by_offset.iter().enumerate() {
        assert_eq!(chunk.file_name, "plate.goo");
        assert_eq!(field(chunk, "Uuid"), id, "one upload");
        assert_eq!(field(chunk, "Offset"), (k * CHUNK_BYTES).to_string());
        assert_eq!(field(chunk, "TotalSize"), data.len().to_string());
        assert_eq!(field(chunk, "Check"), "1");
        assert_eq!(field(chunk, "S-File-MD5"), md5);
        joined.extend_from_slice(&chunk.data);
    }
    assert_eq!(joined, data);
}

/// A printer that refuses a checked upload still gets the file: again from
/// the start, a chunk at a time and unchecked, as every SDCP printer takes it.
#[test]
fn a_printer_that_refuses_the_check_gets_the_file_one_chunk_at_a_time() {
    let data = vec![3u8; CHUNK_BYTES * 3];
    let (addr, got) = web_server(|chunk| {
        if field(chunk, "Check") == "1" {
            (200, r#"{"code":"111111","success":false}"#.to_owned())
        } else {
            (200, OK.to_owned())
        }
    });
    let sent = upload(
        addr,
        "plate.goo",
        &data,
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
    .expect("sent the second time");
    assert_eq!(sent.connections, 1);
    assert!(sent.fell_back.is_some());
    let unchecked: Vec<String> = got
        .lock()
        .unwrap()
        .iter()
        .filter(|c| field(c, "Check") == "0")
        .map(|c| field(c, "Offset"))
        .collect();
    let in_order: Vec<String> = (0..3).map(|k| (k * CHUNK_BYTES).to_string()).collect();
    assert_eq!(unchecked, in_order, "the whole file again, in order");
}

/// A printer keeps the connection open after it answers a chunk: the next
/// chunk goes at once, not when the connection times out (a 33 MB file took
/// 11 minutes so).
#[test]
fn a_printer_that_keeps_the_connection_open_does_not_hold_the_upload() {
    let data = vec![7u8; CHUNK_BYTES * 3];
    let (addr, got) = web_server(|_| (200, OK.to_owned()));
    let start = std::time::Instant::now();
    upload(
        addr,
        "plate.goo",
        &data,
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
    .expect("sent");
    assert_eq!(got.lock().unwrap().len(), 3);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn an_answer_is_whole_by_its_length_or_its_last_chunk() {
    use crate::http::answer_is_whole;
    assert!(!answer_is_whole(
        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nab"
    ));
    assert!(answer_is_whole(
        b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\nabcd"
    ));
    assert!(!answer_is_whole(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n"
    ));
    assert!(answer_is_whole(
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nab\r\n0\r\n\r\n"
    ));
    assert!(answer_is_whole(b"HTTP/1.1 204 No Content\r\n\r\n"));
    assert!(!answer_is_whole(b"HTTP/1.1 200 OK\r\n\r\nuntil it closes"));
    assert!(!answer_is_whole(b"HTTP/1.1 200 OK\r\nContent-Le"));
}

#[test]
fn a_chunk_refused_stops_the_upload_and_says_why() {
    let (addr, _) = web_server(|_| {
        (
            200,
            r#"{"code":"111111","messages":["no space"],"success":false}"#.to_owned(),
        )
    });
    let err = upload(
        addr,
        "x.ctb",
        &[1, 2, 3],
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(
        matches!(&err, Error::Upload(200, said) if said.contains("no space")),
        "{err:?}"
    );
    let (addr, _) = web_server(|_| (500, "oops".to_owned()));
    let err = upload(
        addr,
        "x.ctb",
        &[1, 2, 3],
        &mut |_, _| {},
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Upload(500, _)), "{err:?}");
}

#[test]
fn a_cancelled_upload_sends_nothing() {
    let err = upload(
        "127.0.0.1:9".parse().unwrap(),
        "x.ctb",
        &[0; 10],
        &mut |_, _| {},
        &AtomicBool::new(true),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Cancelled));
}

/// A printer's WebSocket that answers command 128 with `ack`, after a status
/// frame of its own, and hands back what it was asked.
fn websocket_printer(ack: i64) -> (SocketAddr, Arc<Mutex<Option<Value>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let asked = Arc::new(Mutex::new(None));
    let keep = asked.clone();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut socket = tungstenite::accept(stream).unwrap();
        let request: Value = loop {
            if let Message::Text(text) = socket.read().unwrap() {
                break serde_json::from_str(&text).unwrap();
            }
        };
        *keep.lock().unwrap() = Some(request.clone());
        let status = json!({
            "Status": { "CurrentStatus": [1], "PrintInfo": { "CurrentLayer": 12, "TotalLayer": 400, "Filename": "plate.goo" } },
            "Topic": "sdcp/status/abc123def456"
        });
        socket
            .send(Message::Text(status.to_string().into()))
            .unwrap();
        let answer = json!({
            "Id": "x",
            "Data": {
                "Cmd": request["Data"]["Cmd"],
                "Data": { "Ack": ack },
                "RequestID": request["Data"]["RequestID"],
                "MainboardID": "abc123def456",
            },
            "Topic": "sdcp/response/abc123def456"
        });
        socket
            .send(Message::Text(answer.to_string().into()))
            .unwrap();
        let _ = socket.read();
    });
    (addr, asked)
}

#[test]
fn starting_a_print_names_the_file_from_its_first_layer() {
    let (addr, asked) = websocket_printer(0);
    start_print(addr, "abc123def456", "plate.goo").expect("it starts");
    let asked = asked.lock().unwrap().clone().unwrap();
    assert_eq!(asked["Topic"], "sdcp/request/abc123def456");
    assert_eq!(asked["Data"]["Cmd"], 128);
    assert_eq!(asked["Data"]["MainboardID"], "abc123def456");
    assert_eq!(asked["Data"]["Data"]["Filename"], "plate.goo");
    assert_eq!(asked["Data"]["Data"]["StartLayer"], 0);
}

#[test]
fn a_printer_that_refuses_to_start_says_why() {
    let (addr, _) = websocket_printer(1);
    let err = start_print(addr, "abc123def456", "plate.goo").unwrap_err();
    assert!(matches!(err, Error::Refused(Refusal::Busy)), "{err:?}");
    let (addr, _) = websocket_printer(7);
    let err = start_print(addr, "abc123def456", "plate.goo").unwrap_err();
    assert!(matches!(err, Error::Refused(Refusal::Model)), "{err:?}");
}

#[test]
fn the_status_says_what_the_printer_does() {
    let (addr, _) = websocket_printer(0);
    let status = status(addr, "abc123def456").unwrap();
    assert_eq!(
        status,
        Status {
            machine: Machine::Printing,
            layers: Some((12, 400)),
            file: Some("plate.goo".into()),
        }
    );
}

#[test]
fn a_printer_file_name_keeps_what_its_firmware_takes() {
    assert_eq!(
        printer_file_name("Aarakocra Archer - Plate 1.goo"),
        "Aarakocra_Archer_-_Plate_1.goo"
    );
    assert_eq!(printer_file_name("Ñandú.ctb"), "and_.ctb");
    assert_eq!(printer_file_name(".goo"), "print.goo");
    assert_eq!(printer_file_name("plate.ctb"), "plate.ctb");
}

#[test]
fn an_upload_id_is_a_version_4_uuid() {
    let id = upload_id();
    assert_eq!(id.len(), 36, "{id}");
    assert_eq!(&id[14..15], "4", "{id}");
    assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"), "{id}");
    assert_ne!(upload_id(), id);
}

#[test]
fn an_answer_in_chunks_is_put_back_together() {
    let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
    let answer = parse_response(raw).unwrap();
    assert_eq!(answer.status, 200);
    assert_eq!(answer.body, b"Wikipedia");
    let body = multipart_body(
        "B",
        &[],
        &FilePart {
            field: "File",
            file_name: "a\"b.ctb",
            data: b"xy",
        },
    );
    assert!(String::from_utf8_lossy(&body).contains("filename=\"a_b.ctb\""));
}
