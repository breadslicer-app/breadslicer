/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A printer's camera, watched live. The printer turns its stream on with an
//! SDCP command and serves it over RTSP; this reads it in-process (`retina`)
//! and decodes its pictures (H.264 with OpenH264, or JPEG), so nothing is
//! downloaded or run besides the app.
//!
//! A printer streams to only a few viewers at once, and each connection it
//! refuses costs it: a refusal is said, never retried in a loop, and the
//! stream is torn down and turned off when the view closes.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use openh264::decoder::Decoder;
use openh264::formats::YUVSource;
use retina::client::{
    PlayOptions, Session, SessionGroup, SessionOptions, SetupOptions, TeardownPolicy,
};
use retina::codec::{CodecItem, FrameFormat, ParametersRef};

use crate::sdcp::{self, Printer};

/// How long the printer may take to describe its stream.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// How long the first picture may take to come.
const FIRST_PICTURE_TIMEOUT: Duration = Duration::from_secs(12);
/// How long the stream may go quiet once it has shown something.
const QUIET_TIMEOUT: Duration = Duration::from_secs(10);
/// How often the stream is looked at for a stop.
const CANCEL_POLL: Duration = Duration::from_millis(200);
/// How long a closing stream waits for its TEARDOWN.
const TEARDOWN_WAIT: Duration = Duration::from_secs(3);
/// Pictures handed on at most this often (about 15 a second): every one is
/// decoded, since each H.264 picture builds on the ones before.
const SHOW_EVERY: Duration = Duration::from_millis(66);
/// Pictures that may fail to decode in a row before the stream is given up.
const MAX_BAD_PICTURES: usize = 60;

/// One picture, as rows of RGBA pixels from the top.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl fmt::Debug for Picture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Picture({}x{})", self.width, self.height)
    }
}

/// What watching a camera gives, as it comes.
#[derive(Debug)]
pub enum Event {
    /// The printer is streaming: where from and what.
    Playing {
        url: String,
        codec: String,
        width: u32,
        height: u32,
    },
    /// The latest picture.
    Picture(Picture),
}

/// Why the camera could not be watched, or stopped.
#[derive(Debug)]
pub enum CameraError {
    /// The printer says it has no camera.
    NoCamera,
    /// The printer already streams to as many viewers as it allows.
    TooManyViewers,
    /// It refused for a reason this does not know (the command's `Ack`).
    Refused(i64),
    /// The command to turn the stream on failed.
    Printer(sdcp::Error),
    /// The stream could not be opened or broke.
    Stream(String),
    /// The stream carries no video this reads: what it offers.
    NoVideo(Vec<String>),
    /// Its pictures do not decode.
    Codec(String),
    /// Nothing came for too long.
    Silent,
}

impl fmt::Display for CameraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCamera => f.write_str("the printer has no camera"),
            Self::TooManyViewers => {
                f.write_str("the printer already streams to as many viewers as it allows")
            }
            Self::Refused(ack) => write!(f, "the printer refused the camera (code {ack})"),
            Self::Printer(err) => write!(f, "{err}"),
            Self::Stream(what) => write!(f, "the camera stream failed: {what}"),
            Self::NoVideo(offered) => write!(
                f,
                "the camera stream has no video this reads (it offers {})",
                if offered.is_empty() {
                    "nothing".to_owned()
                } else {
                    offered.join(", ")
                }
            ),
            Self::Codec(what) => write!(f, "the camera's pictures do not decode: {what}"),
            Self::Silent => f.write_str("the camera sent nothing for too long"),
        }
    }
}

impl std::error::Error for CameraError {}

/// Watches `printer`'s camera until `cancel` is set or the stream ends,
/// handing every event to `on_event`. Blocks: run it on a worker. The stream
/// is turned off again on the way out, whatever happened.
pub fn watch(
    printer: &Printer,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(Event),
) -> Result<(), CameraError> {
    let answer = sdcp::video(printer.service(), &printer.mainboard_id, true)
        .map_err(CameraError::Printer)?;
    match answer.ack {
        0 => {}
        1 => return Err(CameraError::TooManyViewers),
        2 => return Err(CameraError::NoCamera),
        other => return Err(CameraError::Refused(other)),
    }
    let url = answer.stream_url(printer.ip);
    let watched = watch_stream(&url, cancel, on_event);
    // Best effort: the printer drops the stream on its own when nobody reads it.
    let _ = sdcp::video(printer.service(), &printer.mainboard_id, false);
    watched
}

/// Watches the RTSP stream at `url` until `cancel` is set or it ends.
pub fn watch_stream(
    url: &str,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(Event),
) -> Result<(), CameraError> {
    let parsed = url::Url::parse(url).map_err(|err| CameraError::Stream(err.to_string()))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| CameraError::Stream(err.to_string()))?;
    let group = Arc::new(SessionGroup::default());
    let watched = runtime.block_on(play(parsed, group.clone(), cancel, on_event));
    // The session sends its TEARDOWN as it drops: give it the time to go.
    runtime.block_on(async {
        let _ = tokio::time::timeout(TEARDOWN_WAIT, group.await_teardown()).await;
    });
    watched
}

async fn play(
    url: url::Url,
    group: Arc<SessionGroup>,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(Event),
) -> Result<(), CameraError> {
    let stream_error = |err: retina::Error| CameraError::Stream(err.to_string());
    let text = url.to_string();
    let options = SessionOptions::default()
        .user_agent("Breadslicer".to_owned())
        .session_group(group)
        .teardown(TeardownPolicy::Auto);
    let mut session = tokio::time::timeout(CONNECT_TIMEOUT, Session::describe(url, options))
        .await
        .map_err(|_| CameraError::Silent)?
        .map_err(stream_error)?;
    let offered: Vec<String> = session
        .streams()
        .iter()
        .map(|s| format!("{}/{}", s.media(), s.encoding_name()))
        .collect();
    let Some((index, codec)) = session.streams().iter().enumerate().find_map(|(i, s)| {
        (s.media() == "video")
            .then(|| Codec::of(s.encoding_name()))
            .flatten()
            .map(|codec| (i, codec))
    }) else {
        return Err(CameraError::NoVideo(offered));
    };
    tokio::time::timeout(
        CONNECT_TIMEOUT,
        session.setup(
            index,
            SetupOptions::default().frame_format(FrameFormat::SIMPLE),
        ),
    )
    .await
    .map_err(|_| CameraError::Silent)?
    .map_err(stream_error)?;
    let playing = tokio::time::timeout(CONNECT_TIMEOUT, session.play(PlayOptions::default()))
        .await
        .map_err(|_| CameraError::Silent)?
        .map_err(stream_error)?;
    let mut demuxed = playing.demuxed().map_err(stream_error)?;

    let mut pictures = Pictures::new(codec)?;
    let started = Instant::now();
    let mut last_heard = Instant::now();
    let mut last_shown: Option<Instant> = None;
    let mut playing_told = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        if last_heard.elapsed() > QUIET_TIMEOUT
            || (!playing_told && started.elapsed() > FIRST_PICTURE_TIMEOUT)
        {
            return Err(CameraError::Silent);
        }
        let item = match tokio::time::timeout(CANCEL_POLL, demuxed.next()).await {
            Err(_) => continue,
            Ok(None) => return Ok(()),
            Ok(Some(item)) => item.map_err(stream_error)?,
        };
        let CodecItem::VideoFrame(frame) = item else {
            continue;
        };
        last_heard = Instant::now();
        let due = last_shown.is_none_or(|at| at.elapsed() >= SHOW_EVERY);
        let Some(picture) = pictures.push(frame.data(), due)? else {
            continue;
        };
        if !playing_told {
            playing_told = true;
            let codec = match demuxed.streams()[index].parameters() {
                Some(ParametersRef::Video(params)) => params.rfc6381_codec().to_owned(),
                _ => codec.name().to_owned(),
            };
            on_event(Event::Playing {
                url: text.clone(),
                codec,
                width: picture.width,
                height: picture.height,
            });
        }
        last_shown = Some(Instant::now());
        on_event(Event::Picture(picture));
    }
}

/// What a camera's video is coded in, of what this reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    H264,
    Jpeg,
}

impl Codec {
    fn of(encoding_name: &str) -> Option<Self> {
        match encoding_name {
            "h264" => Some(Self::H264),
            "jpeg" => Some(Self::Jpeg),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Jpeg => "jpeg",
        }
    }
}

/// Turns a stream's frames into pictures.
pub struct Pictures {
    codec: Codec,
    h264: Option<Decoder>,
    bad_in_a_row: usize,
}

impl fmt::Debug for Pictures {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pictures({:?})", self.codec)
    }
}

impl Pictures {
    pub fn new(codec: Codec) -> Result<Self, CameraError> {
        let h264 = match codec {
            Codec::H264 => Some(Decoder::new().map_err(|err| CameraError::Codec(err.to_string()))?),
            Codec::Jpeg => None,
        };
        Ok(Self {
            codec,
            h264,
            bad_in_a_row: 0,
        })
    }

    /// Decodes one frame (H.264 in Annex B with its parameter sets, or one
    /// JPEG) and, when `wanted`, gives its picture. A frame that does not
    /// decode is skipped; too many in a row end the stream.
    pub fn push(&mut self, frame: &[u8], wanted: bool) -> Result<Option<Picture>, CameraError> {
        let decoded = match self.codec {
            Codec::H264 => {
                let decoder = self.h264.as_mut().expect("an H.264 decoder");
                match decoder.decode(frame) {
                    Ok(None) => Ok(None),
                    Ok(Some(yuv)) if wanted => {
                        let (width, height) = yuv.dimensions();
                        let mut rgba = vec![0u8; width * height * 4];
                        yuv.write_rgba8(&mut rgba);
                        Ok(Some(Picture {
                            width: width as u32,
                            height: height as u32,
                            rgba,
                        }))
                    }
                    Ok(Some(_)) => Ok(None),
                    Err(err) => Err(err.to_string()),
                }
            }
            Codec::Jpeg if !wanted => Ok(None),
            Codec::Jpeg => image::load_from_memory_with_format(frame, image::ImageFormat::Jpeg)
                .map(|picture| {
                    let rgba = picture.to_rgba8();
                    Some(Picture {
                        width: rgba.width(),
                        height: rgba.height(),
                        rgba: rgba.into_raw(),
                    })
                })
                .map_err(|err| err.to_string()),
        };
        match decoded {
            Ok(picture) => {
                self.bad_in_a_row = 0;
                Ok(picture)
            }
            Err(what) => {
                self.bad_in_a_row += 1;
                if self.bad_in_a_row > MAX_BAD_PICTURES {
                    Err(CameraError::Codec(what))
                } else {
                    Ok(None)
                }
            }
        }
    }
}
