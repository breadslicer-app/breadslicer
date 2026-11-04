/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Typed refusals of the renderer's GPU resources.

use std::fmt;

use slint::wgpu_30::wgpu;

use crate::i18n::ctr;

/// The part of the renderer whose construction the device refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererStage {
    Mesh,
    SurfacePaint,
    Outline,
    GizmoBuffers,
    Gizmo,
    Plate,
    Cap,
    Label,
}

/// A stage that the device refused, with the device's cause on 1 line. Its text
/// is in the language in force each time it is formatted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageRefusal {
    pub stage: RendererStage,
    pub cause: String,
}

impl StageRefusal {
    /// The refusal of `stage`, with the cause of `source`.
    pub fn new(stage: RendererStage, source: &wgpu::Error) -> Self {
        Self {
            stage,
            cause: one_line(source),
        }
    }
}

impl fmt::Display for StageRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cause = &self.cause;
        let text = match self.stage {
            RendererStage::Mesh => ctr!(
                "ErrorDialog",
                "The 3D view could not build its mesh pipeline: {}",
                cause
            ),
            RendererStage::SurfacePaint => ctr!(
                "ErrorDialog",
                "The 3D view could not build its surface paint pipeline: {}",
                cause
            ),
            RendererStage::Outline => ctr!(
                "ErrorDialog",
                "The 3D view could not build its outline pipeline: {}",
                cause
            ),
            RendererStage::GizmoBuffers => ctr!(
                "ErrorDialog",
                "The 3D view could not build its gizmo geometry: {}",
                cause
            ),
            RendererStage::Gizmo => ctr!(
                "ErrorDialog",
                "The 3D view could not build its gizmo pipeline: {}",
                cause
            ),
            RendererStage::Plate => ctr!(
                "ErrorDialog",
                "The 3D view could not build its build plate pipeline: {}",
                cause
            ),
            RendererStage::Cap => ctr!(
                "ErrorDialog",
                "The 3D view could not build its slice cap pipeline: {}",
                cause
            ),
            RendererStage::Label => ctr!(
                "ErrorDialog",
                "The 3D view could not build its plate label pipeline: {}",
                cause
            ),
        };
        f.write_str(&text)
    }
}

/// `Renderer::new` refused: the device rejected one of the renderer's static
/// resources. `stage` names the first one refused.
#[derive(Debug)]
pub struct RendererInitError {
    pub stage: RendererStage,
    pub source: wgpu::Error,
}

impl fmt::Display for RendererInitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        StageRefusal::new(self.stage, &self.source).fmt(f)
    }
}

impl std::error::Error for RendererInitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// A viewport-sized or scene-dependent allocation the renderer makes while
/// drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderAllocation {
    PlateGeometry,
    MsaaTargets,
    ColourTargets,
    CapTexture,
    LabelTexture,
    ReadbackBuffer,
}

/// An allocation that the device refused, with the device's cause on 1 line.
/// Its text is in the language in force each time it is formatted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocationRefusal {
    pub what: RenderAllocation,
    pub cause: String,
}

impl AllocationRefusal {
    /// The refusal of `what`, with the cause of `source`.
    pub fn new(what: RenderAllocation, source: &wgpu::Error) -> Self {
        Self {
            what,
            cause: one_line(source),
        }
    }
}

impl fmt::Display for AllocationRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let cause = &self.cause;
        let text = match self.what {
            RenderAllocation::PlateGeometry => ctr!(
                "ErrorDialog",
                "The graphics device refused the build plate geometry: {}",
                cause
            ),
            RenderAllocation::MsaaTargets => ctr!(
                "ErrorDialog",
                "The graphics device refused the multisample buffers: {}",
                cause
            ),
            RenderAllocation::ColourTargets => ctr!(
                "ErrorDialog",
                "The graphics device refused the output image: {}",
                cause
            ),
            RenderAllocation::CapTexture => ctr!(
                "ErrorDialog",
                "The graphics device refused the slice cap texture: {}",
                cause
            ),
            RenderAllocation::LabelTexture => ctr!(
                "ErrorDialog",
                "The graphics device refused the plate label texture: {}",
                cause
            ),
            RenderAllocation::ReadbackBuffer => ctr!(
                "ErrorDialog",
                "The graphics device refused the image readback buffer: {}",
                cause
            ),
        };
        f.write_str(&text)
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
    /// The device is gone; nothing was attempted.
    DeviceLost,
    /// A drawn frame never made it back to the CPU.
    ReadbackFailed { reason: String },
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // A refusal is the only render error that a dialog shows.
            Self::DeviceRefused { what, source } => AllocationRefusal::new(*what, source).fmt(f),
            Self::DeviceLost => f.write_str("The graphics device was lost."),
            Self::ReadbackFailed { reason } => {
                write!(f, "The rendered image could not be read back: {reason}")
            }
        }
    }
}

impl std::error::Error for RenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::DeviceRefused { source, .. } => Some(source),
            Self::DeviceLost | Self::ReadbackFailed { .. } => None,
        }
    }
}

/// wgpu's `Display` is a multi-line "Validation Error\n\nCaused by:\n..." tree
/// with the deepest cause on the last line; a 1-line message wants the class
/// and that cause.
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

    /// A validation error whose description has wgpu's real shape: class line,
    /// blank, "Caused by:", context line, deepest cause.
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
    fn each_stage_has_its_own_sentence() {
        let sentence = |stage| {
            RendererInitError {
                stage,
                source: validation("x"),
            }
            .to_string()
        };
        assert_eq!(
            sentence(RendererStage::Mesh),
            "The 3D view could not build its mesh pipeline: Validation Error: x"
        );
        assert_eq!(
            sentence(RendererStage::SurfacePaint),
            "The 3D view could not build its surface paint pipeline: Validation Error: x"
        );
        assert_eq!(
            sentence(RendererStage::Outline),
            "The 3D view could not build its outline pipeline: Validation Error: x"
        );
        assert_eq!(
            sentence(RendererStage::GizmoBuffers),
            "The 3D view could not build its gizmo geometry: Validation Error: x"
        );
        assert_eq!(
            sentence(RendererStage::Gizmo),
            "The 3D view could not build its gizmo pipeline: Validation Error: x"
        );
        assert_eq!(
            sentence(RendererStage::Plate),
            "The 3D view could not build its build plate pipeline: Validation Error: x"
        );
        assert_eq!(
            sentence(RendererStage::Cap),
            "The 3D view could not build its slice cap pipeline: Validation Error: x"
        );
        assert_eq!(
            sentence(RendererStage::Label),
            "The 3D view could not build its plate label pipeline: Validation Error: x"
        );
    }

    #[test]
    fn each_allocation_has_its_own_sentence() {
        let sentence = |what| {
            RenderError::DeviceRefused {
                what,
                source: validation("x"),
            }
            .to_string()
        };
        assert_eq!(
            sentence(RenderAllocation::PlateGeometry),
            "The graphics device refused the build plate geometry: Validation Error: x"
        );
        assert_eq!(
            sentence(RenderAllocation::MsaaTargets),
            "The graphics device refused the multisample buffers: Validation Error: x"
        );
        assert_eq!(
            sentence(RenderAllocation::ColourTargets),
            "The graphics device refused the output image: Validation Error: x"
        );
        assert_eq!(
            sentence(RenderAllocation::CapTexture),
            "The graphics device refused the slice cap texture: Validation Error: x"
        );
        assert_eq!(
            sentence(RenderAllocation::LabelTexture),
            "The graphics device refused the plate label texture: Validation Error: x"
        );
        assert_eq!(
            sentence(RenderAllocation::ReadbackBuffer),
            "The graphics device refused the image readback buffer: Validation Error: x"
        );
    }

    #[test]
    fn a_lost_device_has_no_source_and_says_so() {
        use std::error::Error as _;
        let err = RenderError::DeviceLost;
        assert!(err.source().is_none());
        assert!(err.to_string().contains("lost"), "{err}");
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
