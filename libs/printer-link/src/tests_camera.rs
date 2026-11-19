/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A printer's camera against a camera played on this machine: an RTSP server
//! that streams real H.264, coded here with OpenH264's encoder, and the
//! printer's answer to the camera command.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use openh264::encoder::Encoder;
use openh264::formats::{RgbSliceU8, YUVBuffer};
use serde_json::{Value, json};
use tungstenite::Message;

use crate::camera::*;
use crate::sdcp::{VideoAnswer, video, video_of};

const SIDE: usize = 64;

/// A solid picture of `rgb`, coded as H.264 in Annex B (an IDR picture
/// carries its SPS and PPS).
fn coded_frames(rgb: [u8; 3], count: usize) -> Vec<Vec<u8>> {
    let mut encoder = Encoder::new().unwrap();
    let pixels: Vec<u8> = (0..SIDE * SIDE).flat_map(|_| rgb).collect();
    let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(&pixels, (SIDE, SIDE)));
    (0..count)
        .map(|_| encoder.encode(&yuv).unwrap().to_vec())
        .collect()
}

/// The NAL units of an Annex B frame, without their start codes.
fn nal_units(frame: &[u8]) -> Vec<Vec<u8>> {
    openh264::nal_units(frame)
        .map(|unit| {
            let start = unit.iter().position(|&b| b == 1).unwrap() + 1;
            unit[start..].to_vec()
        })
        .filter(|unit| !unit.is_empty())
        .collect()
}

fn near(a: &[u8], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(&x, y)| x.abs_diff(y) <= 12)
}

#[test]
fn coded_pictures_decode_to_what_was_coded() {
    let mut pictures = Pictures::new(Codec::H264).unwrap();
    let shown: Vec<Picture> = coded_frames([200, 30, 40], 5)
        .iter()
        .filter_map(|frame| pictures.push(frame, true).unwrap())
        .collect();
    assert!(!shown.is_empty());
    for picture in &shown {
        assert_eq!((picture.width, picture.height), (SIDE as u32, SIDE as u32));
        assert_eq!(picture.rgba.len(), SIDE * SIDE * 4);
        let middle = (SIDE / 2 * SIDE + SIDE / 2) * 4;
        assert!(
            near(&picture.rgba[middle..middle + 3], [200, 30, 40]),
            "{:?}",
            &picture.rgba[middle..middle + 4]
        );
    }
}

#[test]
fn a_picture_not_wanted_is_decoded_but_not_given() {
    let mut pictures = Pictures::new(Codec::H264).unwrap();
    let frames = coded_frames([10, 200, 10], 3);
    assert_eq!(pictures.push(&frames[0], false).unwrap(), None);
    // The next one builds on it all the same.
    assert!(pictures.push(&frames[1], true).unwrap().is_some());
}

#[test]
fn garbage_is_skipped_until_there_is_too_much_of_it() {
    let mut pictures = Pictures::new(Codec::Jpeg).unwrap();
    for _ in 0..60 {
        assert_eq!(pictures.push(b"not a picture", true).unwrap(), None);
    }
    assert!(matches!(
        pictures.push(b"not a picture", true),
        Err(CameraError::Codec(_))
    ));
}

/// What the played camera saw.
#[derive(Default)]
struct Seen {
    methods: Vec<String>,
    /// The Transport header of each SETUP.
    transports: Vec<String>,
}

/// How a played camera streams.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Serves {
    /// Over the RTSP connection (interleaved RTP); a request for UDP is
    /// refused as an unsupported transport.
    Tcp,
    /// To the client's UDP ports; a request for TCP is answered as the
    /// printers' boards do, without the channels to read it on.
    Udp,
}

/// Two UDP sockets on adjacent ports, RTP on the even one.
fn udp_pair() -> (UdpSocket, u16) {
    for _ in 0..50 {
        let rtp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = rtp.local_addr().unwrap().port();
        if port % 2 != 0 || port == u16::MAX {
            continue;
        }
        if let Ok(rtcp) = UdpSocket::bind(("127.0.0.1", port + 1)) {
            // Kept open for the run, so the client's RTCP finds it.
            std::mem::forget(rtcp);
            return (rtp, port);
        }
    }
    panic!("no two adjacent UDP ports");
}

/// An RTSP camera on this machine: answers DESCRIBE, SETUP and PLAY on any
/// number of connections, then streams `frames` (one NAL unit a packet, as
/// packetization mode 1 allows for small units), 25 a second, over and over.
fn rtsp_camera(frames: Vec<Vec<u8>>, serves: Serves) -> (String, Arc<Mutex<Seen>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("rtsp://127.0.0.1:{port}/video");
    let seen = Arc::new(Mutex::new(Seen::default()));
    let keep = seen.clone();
    let base = url.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let (keep, base, frames) = (keep.clone(), base.clone(), frames.clone());
            thread::spawn(move || rtsp_connection(stream, serves, &keep, &base, frames));
        }
    });
    (url, seen)
}

fn rtsp_connection(
    stream: TcpStream,
    serves: Serves,
    keep: &Mutex<Seen>,
    base: &str,
    frames: Vec<Vec<u8>>,
) {
    let writer = Arc::new(Mutex::new(stream.try_clone().unwrap()));
    let mut reader = BufReader::new(stream);
    let playing = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    // Where the client reads datagrams, once it set up over UDP.
    let mut udp_to: Option<(Arc<UdpSocket>, u16)> = None;
    loop {
        // Interleaved data from the client (RTCP reports) is skipped.
        let mut first = [0u8; 1];
        if reader.read_exact(&mut first).is_err() {
            break;
        }
        if first[0] == b'$' {
            let mut head = [0u8; 3];
            reader.read_exact(&mut head).unwrap();
            let len = u16::from_be_bytes([head[1], head[2]]) as usize;
            let mut body = vec![0u8; len];
            reader.read_exact(&mut body).unwrap();
            continue;
        }
        let mut head = vec![first[0]];
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                stop.store(true, Ordering::Relaxed);
                return;
            }
            head.extend_from_slice(line.as_bytes());
            if line == "\r\n" {
                break;
            }
        }
        let text = String::from_utf8(head).unwrap();
        let method = text.split(' ').next().unwrap().to_owned();
        let header = |name: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(name))
                .unwrap_or("")
                .trim()
                .to_owned()
        };
        let cseq = header("CSeq: ");
        keep.lock().unwrap().methods.push(method.clone());
        let reply = match method.as_str() {
            "OPTIONS" => format!(
                "RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nPublic: DESCRIBE, SETUP, PLAY, TEARDOWN\r\n\r\n"
            ),
            "DESCRIBE" => {
                let sdp = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=Camera\r\nt=0 0\r\n\
                           m=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n\
                           a=fmtp:96 packetization-mode=1\r\na=control:trackID=0\r\n";
                format!(
                    "RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nContent-Base: {base}/\r\n\
                     Content-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{sdp}",
                    sdp.len()
                )
            }
            "SETUP" => {
                let asked = header("Transport: ");
                keep.lock().unwrap().transports.push(asked.clone());
                let over_tcp = asked.contains("TCP");
                match (serves, over_tcp) {
                    (Serves::Tcp, true) => format!(
                        "RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nSession: 12345678;timeout=60\r\n\
                         Transport: RTP/AVP/TCP;unicast;interleaved=0-1;ssrc=0BADCAFE\r\n\r\n"
                    ),
                    (Serves::Tcp, false) => {
                        format!("RTSP/1.0 461 Unsupported Transport\r\nCSeq: {cseq}\r\n\r\n")
                    }
                    (Serves::Udp, true) => format!(
                        "RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nSession: 12345678;timeout=60\r\n\
                         Transport: RTP/AVP;unicast;server_port=6970-6971\r\n\r\n"
                    ),
                    (Serves::Udp, false) => {
                        let client: u16 = asked
                            .split(';')
                            .find_map(|p| p.strip_prefix("client_port="))
                            .and_then(|p| p.split('-').next())
                            .and_then(|p| p.parse().ok())
                            .expect("a client port");
                        let (socket, server) = udp_pair();
                        udp_to = Some((Arc::new(socket), client));
                        format!(
                            "RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nSession: 12345678;timeout=60\r\n\
                             Transport: RTP/AVP;unicast;client_port={client}-{};\
                             server_port={server}-{};ssrc=0BADCAFE\r\n\r\n",
                            client + 1,
                            server + 1
                        )
                    }
                }
            }
            "PLAY" => format!("RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nSession: 12345678\r\n\r\n"),
            "TEARDOWN" => {
                stop.store(true, Ordering::Relaxed);
                format!("RTSP/1.0 200 OK\r\nCSeq: {cseq}\r\nSession: 12345678\r\n\r\n")
            }
            _ => format!("RTSP/1.0 501 Not Implemented\r\nCSeq: {cseq}\r\n\r\n"),
        };
        if writer.lock().unwrap().write_all(reply.as_bytes()).is_err() {
            break;
        }
        if method == "PLAY" && !playing.swap(true, Ordering::Relaxed) {
            let (stop, frames) = (stop.clone(), frames.clone());
            match udp_to.clone() {
                Some((socket, client)) => {
                    thread::spawn(move || {
                        stream_frames(&stop, &frames, |packet| {
                            socket.send_to(packet, ("127.0.0.1", client)).is_ok()
                        })
                    });
                }
                None => {
                    let writer = writer.clone();
                    thread::spawn(move || {
                        stream_frames(&stop, &frames, |packet| {
                            let mut framed = vec![b'$', 0];
                            framed.extend_from_slice(&(packet.len() as u16).to_be_bytes());
                            framed.extend_from_slice(packet);
                            writer.lock().unwrap().write_all(&framed).is_ok()
                        })
                    });
                }
            }
        }
        if method == "TEARDOWN" {
            break;
        }
    }
    stop.store(true, Ordering::Relaxed);
}

/// Sends `frames` as RTP packets through `send` until `stop` or it fails.
fn stream_frames(stop: &AtomicBool, frames: &[Vec<u8>], mut send: impl FnMut(&[u8]) -> bool) {
    let mut seq: u16 = 0;
    let mut timestamp: u32 = 0;
    for frame in frames.iter().cycle() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let units = nal_units(frame);
        for (i, unit) in units.iter().enumerate() {
            let marker = if i + 1 == units.len() { 0x80 } else { 0 };
            let mut packet = vec![0x80, marker | 96];
            packet.extend_from_slice(&seq.to_be_bytes());
            packet.extend_from_slice(&timestamp.to_be_bytes());
            packet.extend_from_slice(&0x0BAD_CAFEu32.to_be_bytes());
            packet.extend_from_slice(unit);
            if !send(&packet) {
                return;
            }
            seq = seq.wrapping_add(1);
        }
        timestamp = timestamp.wrapping_add(3600);
        thread::sleep(Duration::from_millis(40));
    }
}

/// Watches the camera at `url` for four pictures; what it said it played,
/// and the pictures.
fn watch_four(url: &str) -> ((String, String, u32, u32), Vec<Picture>) {
    let cancel = AtomicBool::new(false);
    let mut playing = None;
    let mut pictures = Vec::new();
    let start = Instant::now();
    watch_stream(url, &cancel, &mut |event| match event {
        Event::Playing {
            transport,
            codec,
            width,
            height,
            ..
        } => playing = Some((transport.to_owned(), codec, width, height)),
        Event::Picture(picture) => {
            pictures.push(picture);
            if pictures.len() >= 4 {
                cancel.store(true, Ordering::Relaxed);
            }
        }
    })
    .expect("it watches until closed");
    assert!(
        start.elapsed() < Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    (playing.expect("it says it is playing"), pictures)
}

fn torn_down(seen: &Mutex<Seen>) {
    let deadline = Instant::now() + Duration::from_secs(4);
    while !seen.lock().unwrap().methods.iter().any(|m| m == "TEARDOWN") {
        assert!(
            Instant::now() < deadline,
            "{:?}",
            seen.lock().unwrap().methods
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_camera_stream_shows_its_pictures_and_is_torn_down_when_closed() {
    let frames = coded_frames([30, 60, 220], 10);
    assert!(
        frames
            .iter()
            .all(|f| nal_units(f).iter().all(|u| u.len() < 1400))
    );
    // A camera that streams only over the RTSP connection: UDP is asked
    // first, refused, then TCP.
    let (url, seen) = rtsp_camera(frames, Serves::Tcp);
    let ((transport, codec, width, height), pictures) = watch_four(&url);
    assert_eq!(transport, "tcp");
    assert!(codec.starts_with("avc1."), "{codec}");
    assert_eq!((width, height), (SIDE as u32, SIDE as u32));
    let middle = (SIDE / 2 * SIDE + SIDE / 2) * 4;
    assert!(near(&pictures[3].rgba[middle..middle + 3], [30, 60, 220]));
    // The camera is told the stream is over.
    torn_down(&seen);
}

#[test]
fn a_printer_that_streams_over_udp_alone_is_watched_over_udp() {
    // The tester's Mars 5 Ultra answered a SETUP over TCP without the
    // interleaved channels: a board that streams only to UDP ports.
    let (url, seen) = rtsp_camera(coded_frames([220, 120, 20], 10), Serves::Udp);
    let ((transport, _, width, height), pictures) = watch_four(&url);
    assert_eq!(transport, "udp");
    assert_eq!((width, height), (SIDE as u32, SIDE as u32));
    let middle = (SIDE / 2 * SIDE + SIDE / 2) * 4;
    assert!(near(&pictures[3].rgba[middle..middle + 3], [220, 120, 20]));
    torn_down(&seen);
    // One SETUP, over UDP: the printer is not asked twice.
    let asked = seen.lock().unwrap().transports.clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].contains("client_port="), "{asked:?}");
}

#[test]
fn a_camera_that_sends_nothing_is_given_up_on() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        thread::sleep(Duration::from_secs(30));
    });
    let err = watch_stream(
        &format!("rtsp://127.0.0.1:{port}/video"),
        &AtomicBool::new(false),
        &mut |_| {},
    )
    .unwrap_err();
    assert!(matches!(err, CameraError::Silent), "{err:?}");
}

/// A printer's WebSocket that answers the camera command with `data`.
fn camera_printer(data: Value) -> (std::net::SocketAddr, Arc<Mutex<Option<Value>>>) {
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
        let answer = json!({
            "Id": "x",
            "Data": {
                "Cmd": request["Data"]["Cmd"],
                "Data": data,
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
fn the_camera_is_turned_on_with_command_386_and_says_where_it_streams() {
    let (addr, asked) = camera_printer(json!({ "Ack": 0, "VideoUrl": "192.168.1.30:554/video" }));
    let answer = video(addr, "abc123def456", true).unwrap();
    assert_eq!(
        answer,
        VideoAnswer {
            ack: 0,
            url: Some("rtsp://192.168.1.30:554/video".into())
        }
    );
    let asked = asked.lock().unwrap().clone().unwrap();
    assert_eq!(asked["Data"]["Cmd"], 386);
    assert_eq!(asked["Data"]["Data"]["Enable"], 1);

    let (addr, asked) = camera_printer(json!({ "Ack": 1 }));
    let answer = video(addr, "abc123def456", false).unwrap();
    assert_eq!(answer.ack, 1);
    assert_eq!(
        answer.stream_url(Ipv4Addr::new(10, 0, 0, 7)),
        "rtsp://10.0.0.7:554/video"
    );
    assert_eq!(
        asked.lock().unwrap().clone().unwrap()["Data"]["Data"]["Enable"],
        0
    );
}

#[test]
fn a_camera_answer_reads_only_its_own_request() {
    let frame = json!({ "Data": { "RequestID": "a", "Data": { "Ack": 2 } } });
    assert_eq!(video_of(&frame, "b"), None);
    assert_eq!(
        video_of(&frame, "a"),
        Some(VideoAnswer { ack: 2, url: None })
    );
}
