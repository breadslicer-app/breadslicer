# retina 0.4.22, with a tolerant RTSP head reader

A copy of [retina](https://github.com/scottlamb/retina) 0.4.22 (Scott Lamb and
contributors; MIT OR Apache-2.0), as published on crates.io, patched into the
build with `[patch.crates-io]` in the root `Cargo.toml`.

Changed (marked "Breadslicer"):

In `src/client/parse.rs`:

- an encoding whose payload format allows a single clock rate (H.264, H.265,
  JPEG: 90000) takes it whatever the SDP's rtpmap says: the same printer
  declares H.264 at 9000.

In `src/rtsp/parse.rs`:

- a header line may end in a bare LF, and the head may end in one;
- a header name may have blanks around it;
- a header line with an empty value, a name that is not a token or no colon is
  read past instead of failing the whole message.

Why: a printer's camera (an Elegoo Mars 5 Ultra, on a Chitu board) answered
PLAY with a head the crate refused, so its stream never started. ffmpeg reads
such heads.

To drop this copy: delete the folder and the `[patch.crates-io]` entry, once a
retina release reads such heads.
