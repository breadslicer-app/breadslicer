/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The machine and the build, for `system.txt` in a diagnostics file: what
//! a problem with the graphics or the screen needs to be reproduced.

use std::fmt::Write as _;
use std::sync::OnceLock;

use slint::winit_030::WinitWindowAccessor;

/// The graphics adapter this session runs on, as wgpu reports it.
static GRAPHICS: OnceLock<String> = OnceLock::new();

/// Keeps the adapter the session runs on, for the diagnostics file.
pub(crate) fn record_graphics(info: &wgpu::AdapterInfo) {
    let line = format!(
        "{} ({:?}, {:?}), driver {} {}, vendor 0x{:04x}, device 0x{:04x}",
        info.name,
        info.device_type,
        info.backend,
        info.driver,
        info.driver_info,
        info.vendor,
        info.device
    );
    crate::breadcrumbs::note(&format!("gpu: {line}"));
    let _ = GRAPHICS.set(line);
}

/// The screens of a window, read on the UI thread.
#[derive(Debug, Default)]
pub(crate) struct ScreenFacts {
    /// The window's size in physical pixels and its scale factor.
    pub(crate) window: Option<(u32, u32, f32)>,
    /// Each monitor: size in physical pixels, scale factor and refresh rate
    /// in Hz, when known.
    pub(crate) monitors: Vec<(u32, u32, f64, Option<f64>)>,
}

impl ScreenFacts {
    /// The facts of `window` and the monitors it can show on.
    pub(crate) fn of(window: &slint::Window) -> Self {
        let size = window.size();
        let monitors = window
            .with_winit_window(|w| {
                w.available_monitors()
                    .map(|m| {
                        let size = m.size();
                        let hz = m
                            .refresh_rate_millihertz()
                            .map(|mhz| f64::from(mhz) / 1000.0);
                        (size.width, size.height, m.scale_factor(), hz)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            window: Some((size.width, size.height, window.scale_factor())),
            monitors,
        }
    }
}

/// The text of `system.txt`. `exported` is the time of the export, in UTC.
pub(crate) fn report(
    exported: &str,
    language: &str,
    screens: &ScreenFacts,
    log_on: bool,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Breadslicer diagnostics");
    let _ = writeln!(out, "exported: {exported} UTC");
    let _ = writeln!(out, "version: {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(out, "commit: {}", super::commit());
    let _ = writeln!(
        out,
        "build: {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    let _ = writeln!(
        out,
        "system: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    if let Some(version) = platform::os_version() {
        let _ = writeln!(out, "system version: {version}");
    }
    if let Ok(cpus) = std::thread::available_parallelism() {
        let _ = writeln!(out, "processor threads: {cpus}");
    }
    let _ = writeln!(out, "language: {language}");
    let _ = writeln!(
        out,
        "graphics: {}",
        GRAPHICS.get().map_or("not recorded", String::as_str)
    );
    match screens.window {
        Some((w, h, scale)) => {
            let _ = writeln!(out, "window: {w} x {h} px, scale {scale}");
        }
        None => {
            let _ = writeln!(out, "window: not recorded");
        }
    }
    for (i, (w, h, scale, hz)) in screens.monitors.iter().enumerate() {
        let hz = hz.map_or_else(String::new, |hz| format!(", {hz:.0} Hz"));
        let _ = writeln!(out, "monitor {}: {w} x {h} px, scale {scale}{hz}", i + 1);
    }
    let _ = writeln!(out, "diagnostic log: {}", if log_on { "on" } else { "off" });
    out
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

    // The calls into the Win32 registry API.
    #[allow(unsafe_code)]
    fn read(name: &str, flags: u32, data: &mut [u8]) -> Option<usize> {
        let (key, name) = (wide(CURRENT_VERSION), wide(name));
        let mut size = u32::try_from(data.len()).ok()?;
        // SAFETY: the key and value names are NUL-terminated and live for the
        // whole call; `data` is writable for `size` bytes, and the call writes
        // at most that many and puts the count it wrote back in `size`.
        let status = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                name.as_ptr(),
                flags,
                std::ptr::null_mut(),
                data.as_mut_ptr().cast(),
                &mut size,
            )
        };
        (status == 0).then_some(size as usize)
    }

    fn text(name: &str) -> Option<String> {
        let mut data = [0u8; 512];
        let len = read(name, RRF_RT_REG_SZ, &mut data)?;
        let units: Vec<u16> = data[..len]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|&unit| unit != 0)
            .collect();
        Some(String::from_utf16_lossy(&units))
    }

    fn number(name: &str) -> Option<u32> {
        let mut data = [0u8; 4];
        read(name, RRF_RT_REG_DWORD, &mut data)?;
        Some(u32::from_le_bytes(data))
    }

    /// The edition, the release and the build, such as "Windows 10 Pro,
    /// 25H2, build 26200.6899". Windows 11 still calls itself Windows 10
    /// here; a build of 22000 or more is Windows 11.
    pub(super) fn os_version() -> Option<String> {
        let product = text("ProductName")?;
        let release = text("DisplayVersion").or_else(|| text("ReleaseId"));
        let build = text("CurrentBuildNumber");
        let mut out = product;
        if let Some(release) = release {
            out.push_str(&format!(", {release}"));
        }
        if let Some(build) = build {
            out.push_str(&format!(", build {build}"));
            if let Some(revision) = number("UBR") {
                out.push_str(&format!(".{revision}"));
            }
        }
        Some(out)
    }
}

#[cfg(target_os = "linux")]
mod platform {
    /// `PRETTY_NAME` from `/etc/os-release`.
    pub(super) fn os_version() -> Option<String> {
        let release = std::fs::read_to_string("/etc/os-release").ok()?;
        release.lines().find_map(|line| {
            let name = line.strip_prefix("PRETTY_NAME=")?;
            Some(name.trim_matches('"').to_owned())
        })
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod platform {
    pub(super) fn os_version() -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_report_names_the_build_the_screens_and_the_log() {
        let screens = ScreenFacts {
            window: Some((1600, 900, 1.25)),
            monitors: vec![(2560, 1440, 1.25, Some(164.9)), (1920, 1080, 1.0, None)],
        };
        let text = report("2026-10-29 21:14", "es", &screens, true);
        assert!(text.contains(&format!("version: {}", env!("CARGO_PKG_VERSION"))));
        assert!(text.contains("commit: "));
        assert!(text.contains("window: 1600 x 900 px, scale 1.25"));
        assert!(text.contains("monitor 1: 2560 x 1440 px, scale 1.25, 165 Hz"));
        assert!(text.contains("monitor 2: 1920 x 1080 px, scale 1\n"));
        assert!(text.contains("diagnostic log: on"));
        assert!(text.contains("language: es"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_tells_its_version() {
        let version = platform::os_version().expect("the registry has the version");
        assert!(version.starts_with("Windows"), "{version}");
        assert!(version.contains("build "), "{version}");
    }
}
