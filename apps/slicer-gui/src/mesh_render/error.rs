/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Typed refusals of the renderer's GPU resources.

use std::fmt;

use slint::wgpu_29::wgpu;

/// The part of the renderer whose construction the device refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererStage {
    GizmoBuffers,
}

impl fmt::Display for RendererStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::GizmoBuffers => "gizmo geometry",
        })
    }
}

/// `Renderer::new` refused: the device rejected one of the renderer's
/// static resources. `stage` names the first one refused.
#[derive(Debug)]
pub struct RendererInitError {
    pub stage: RendererStage,
    pub source: wgpu::Error,
}

impl fmt::Display for RendererInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "The 3D view could not build its {}: {}",
            self.stage,
            one_line(&self.source)
        )
    }
}

impl std::error::Error for RendererInitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// A viewport-sized or scene-dependent allocation the renderer makes
/// while drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderAllocation {
    PlateGeometry,
}

impl fmt::Display for RenderAllocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::PlateGeometry => "build plate geometry",
        })
    }
}

/// A frame could not be drawn.
#[derive(Debug)]
pub enum RenderError {
    /// The device refused an allocation the frame needs.
    DeviceRefused {
        what: RenderAllocation,
        source: wgpu::Error,
    },
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceRefused { what, source } => write!(
                f,
                "The graphics device refused the {what}: {}",
                one_line(source)
            ),
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DeviceRefused { source, .. } => Some(source),
        }
    }
}

/// wgpu's `Display` is a multi-line "Validation Error\n\nCaused by:\n..."
/// tree with the deepest cause on the last line; a one-line message
/// wants the class and that cause.
fn one_line(err: &wgpu::Error) -> String {
    let text = err.to_string();
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let class = lines.next().unwrap_or_default();
    match lines.next_back() {
        Some(cause) => format!("{class}: {cause}"),
        None => class.to_string(),
    }
}

#[cfg(test)]
mod error_display_tests {
    use super::*;

    /// A validation error whose description has wgpu's real shape:
    /// class line, blank, "Caused by:", context line, deepest cause.
    fn validation(cause: &str) -> wgpu::Error {
        let description = format!(
            "Validation Error\n\nCaused by:\n  In Device::create_buffer, label = 'x'\n    {cause}\n"
        );
        wgpu::Error::Validation {
            source: Box::new(std::io::Error::other(cause.to_string())),
            description,
        }
    }

    #[test]
    fn init_error_names_the_stage_and_the_cause() {
        let err = RendererInitError {
            stage: RendererStage::GizmoBuffers,
            source: validation("Buffer size 2048 is greater than the maximum buffer size (64)"),
        };
        let text = err.to_string();
        assert!(text.contains("gizmo geometry"), "{text}");
        assert!(
            text.contains("Validation Error: Buffer size 2048"),
            "{text}"
        );
        assert!(!text.contains("In Device::create_buffer"), "{text}");
        assert!(!text.contains('\n'), "{text}");
    }

    #[test]
    fn render_error_names_the_allocation_and_the_cause() {
        let err = RenderError::DeviceRefused {
            what: RenderAllocation::PlateGeometry,
            source: validation("out of memory"),
        };
        let text = err.to_string();
        assert!(text.contains("build plate geometry"), "{text}");
        assert!(text.contains("out of memory"), "{text}");
        assert!(!text.contains('\n'), "{text}");
    }

    #[test]
    fn errors_expose_the_wgpu_error_as_source() {
        use std::error::Error as _;
        let init = RendererInitError {
            stage: RendererStage::GizmoBuffers,
            source: validation("x"),
        };
        assert!(init.source().is_some());
        let render = RenderError::DeviceRefused {
            what: RenderAllocation::PlateGeometry,
            source: validation("x"),
        };
        assert!(render.source().is_some());
    }
}
