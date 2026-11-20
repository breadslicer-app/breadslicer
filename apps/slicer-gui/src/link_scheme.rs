/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! `breadslicer://` links (#114): a link on a web page, in Discord or in a
//! message starts Breadslicer, or hands the link to the window already open
//! (through [`crate::single_instance`], like a file).
//!
//! What a link can ask:
//! - `breadslicer://` or `breadslicer://open`: show the app.
//! - `breadslicer://open?file=<path>`: open a file on this computer, as the
//!   command line opens it (a model joins the scene, a project opens in a
//!   window of its own). The path is percent-encoded.
//! - `breadslicer://diagnostics`: the Export diagnostics card, to report a
//!   problem.
//! - `breadslicer://preferences`: the Preferences window.
//!
//! A link never downloads anything and never changes a file: the most it
//! does is open one, as the person could from File > Open. Anything else
//! only shows the app.
//!
//! On Windows the app registers the scheme for the person using it (no
//! administrator rights) each time it starts, pointing at the program file
//! that runs, so the link starts the build in use.

use std::ffi::OsStr;
use std::path::PathBuf;

/// The scheme, without the colon.
pub(crate) const SCHEME: &str = "breadslicer";

/// What a link asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LinkAction {
    /// Show the app (bring the window to the front).
    Show,
    /// Open a file, as a command-line argument would.
    OpenFile(PathBuf),
    /// The Export diagnostics card.
    Diagnostics,
    /// The Preferences window.
    Preferences,
    /// A link this version does not know: the app is only shown.
    Unknown(String),
}

/// Whether a command-line argument is a `breadslicer:` link and not a file.
pub(crate) fn is_link(arg: &OsStr) -> bool {
    arg.to_str().is_some_and(|text| {
        text.len() > SCHEME.len()
            && text.is_char_boundary(SCHEME.len() + 1)
            && text[..SCHEME.len() + 1].eq_ignore_ascii_case("breadslicer:")
    })
}

/// Reads a link. Anything not understood is [`LinkAction::Unknown`].
pub(crate) fn parse(link: &str) -> LinkAction {
    let unknown = || LinkAction::Unknown(link.to_owned());
    let Some(rest) = link
        .get(SCHEME.len() + 1..)
        .filter(|_| is_link(OsStr::new(link)))
    else {
        return unknown();
    };
    let rest = rest.trim_start_matches('/');
    let (place, query) = rest.split_once('?').unwrap_or((rest, ""));
    let place = place.trim_end_matches('/').to_ascii_lowercase();
    match place.as_str() {
        "" | "open" => {
            let file = query
                .split('&')
                .filter_map(|pair| pair.split_once('='))
                .find(|(key, _)| key.eq_ignore_ascii_case("file"))
                .map(|(_, value)| value);
            match file {
                None => LinkAction::Show,
                Some(value) => match percent_decoded(value) {
                    Some(path) if !path.is_empty() => LinkAction::OpenFile(PathBuf::from(path)),
                    _ => unknown(),
                },
            }
        }
        "diagnostics" => LinkAction::Diagnostics,
        "preferences" | "settings" => LinkAction::Preferences,
        _ => unknown(),
    }
}

/// `%XX` decoded (and `+` as a space, as web forms send it), as UTF-8.
/// `None` for a broken escape or text that is not UTF-8.
fn percent_decoded(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = text.get(i + 1..i + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

/// Makes `breadslicer://` links start this program, for the person using
/// it. Writes only when the registration is missing or names another
/// program file. A failure is noted and ignored: links are a convenience.
#[cfg(windows)]
pub(crate) fn register() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let command = format!("\"{}\" \"%1\"", exe.display());
    if registry::command() == Some(command.clone()) {
        return;
    }
    match registry::write(&command, &exe.display().to_string()) {
        Ok(()) => crate::breadcrumbs::note("links: breadslicer:// registered"),
        Err(code) => {
            crate::breadcrumbs::note(&format!("links: could not register ({code})"));
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn register() {}

// The registry is reached through its C API.
#[cfg(windows)]
#[allow(unsafe_code)]
mod registry {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
        RegCloseKey, RegCreateKeyExW, RegGetValueW, RegSetValueExW,
    };

    const ROOT: &str = r"Software\Classes\breadslicer";
    const COMMAND: &str = r"Software\Classes\breadslicer\shell\open\command";
    const ICON: &str = r"Software\Classes\breadslicer\DefaultIcon";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// The command links run today, if any.
    pub(super) fn command() -> Option<String> {
        let key = wide(COMMAND);
        let mut size: u32 = 0;
        // SAFETY: the key name is NUL-terminated; a null buffer asks for the size.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        // SAFETY: `buf` holds `size` bytes.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                std::ptr::null(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let end = buf.iter().position(|&unit| unit == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }

    /// Sets the default value (and `extra` values) of a key under HKCU.
    fn set(path: &str, values: &[(&str, &str)]) -> Result<(), u32> {
        let name = wide(path);
        let mut key: HKEY = std::ptr::null_mut();
        // SAFETY: plain registry calls with NUL-terminated strings; the key
        // is closed below.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                name.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                std::ptr::null_mut(),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(status);
        }
        let mut result = Ok(());
        for (value_name, data) in values {
            let value_name = wide(value_name);
            let data = wide(data);
            // SAFETY: `data` is a NUL-terminated UTF-16 string of the length given.
            let status = unsafe {
                RegSetValueExW(
                    key,
                    if value_name.len() == 1 {
                        std::ptr::null()
                    } else {
                        value_name.as_ptr()
                    },
                    0,
                    REG_SZ,
                    data.as_ptr().cast(),
                    (data.len() * 2) as u32,
                )
            };
            if status != ERROR_SUCCESS {
                result = Err(status);
            }
        }
        // SAFETY: `key` was opened above.
        unsafe { RegCloseKey(key) };
        result
    }

    pub(super) fn write(command: &str, exe: &str) -> Result<(), u32> {
        set(ROOT, &[("", "URL:Breadslicer link"), ("URL Protocol", "")])?;
        set(ICON, &[("", &format!("\"{exe}\",0"))])?;
        set(COMMAND, &[("", command)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_is_told_from_a_file() {
        assert!(is_link(OsStr::new("breadslicer://open")));
        assert!(is_link(OsStr::new("BreadSlicer:diagnostics")));
        assert!(!is_link(OsStr::new("breadslicer")));
        assert!(!is_link(OsStr::new("C:/models/breadslicer.stl")));
        assert!(!is_link(OsStr::new("breadslicer.bread")));
    }

    #[test]
    fn links_ask_for_what_they_name() {
        assert_eq!(parse("breadslicer://"), LinkAction::Show);
        assert_eq!(parse("breadslicer://open"), LinkAction::Show);
        assert_eq!(parse("breadslicer://open/"), LinkAction::Show);
        assert_eq!(parse("breadslicer://DIAGNOSTICS"), LinkAction::Diagnostics);
        assert_eq!(parse("breadslicer://preferences/"), LinkAction::Preferences);
        assert_eq!(
            parse("breadslicer://open?file=C%3A%5Cmodels%5Cmy%20mini.stl"),
            LinkAction::OpenFile(PathBuf::from(r"C:\models\my mini.stl"))
        );
        assert_eq!(
            parse("breadslicer:open?x=1&file=D:/a+b.bread"),
            LinkAction::OpenFile(PathBuf::from("D:/a b.bread"))
        );
        assert_eq!(
            parse("breadslicer://open?file=caf%C3%A9.stl"),
            LinkAction::OpenFile(PathBuf::from("café.stl"))
        );
    }

    #[test]
    fn a_link_not_understood_only_shows_the_app() {
        for link in [
            "breadslicer://download?url=https://example.com/x.stl",
            "breadslicer://open?file=%ZZ",
            "breadslicer://open?file=",
            "breadslicer://open?file=%FF",
        ] {
            assert!(
                matches!(parse(link), LinkAction::Unknown(_)),
                "{link} should not be understood"
            );
        }
    }
}
