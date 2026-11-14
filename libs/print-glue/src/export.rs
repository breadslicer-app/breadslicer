/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: Apache-2.0
 */

//! The printer-file export of a finished slice: the full print configuration,
//! its checks against the layers the contours were cut on, and the machine
//! program a printer file carries.

use std::fmt;
use std::num::NonZeroU32;
use std::path::Path;

use sc_print_primitives::{AaLevel, PositiveFiniteMm};
use sc_print_profiles::{
    EffectiveMaterial, PrinterProfile, ResolveError, ResolvedPrintConfig, SceneOverrides, resolve,
    snap_to_z_step,
};
use sc_sl1cer::profile::{LayerSchedule, PackingMode};
use sc_sla_formats::layer_images::lit_layer_count;
use sc_sla_formats::{
    PrintPreviews, PrintProgram, PrinterFileFormat, RasterSetup, WriterError, write_printer_file,
};

use crate::print_program::{UnprojectablePrint, creation_time_now, project_to_print_program};
use crate::projection::project_hardware_to_slice_profile;
use crate::slice_plate::PlateContours;

/// A printer-file export ready to write: the contours, the print configuration
/// they were checked against, and the machine program built from both.
#[derive(Debug)]
pub struct ExportJob {
    pub(crate) contours: PlateContours,
    pub(crate) config: ResolvedPrintConfig,
    pub(crate) program: PrintProgram,
}

impl ExportJob {
    /// Return the resolved print configuration, for a frontend to report the
    /// values no profile supplied.
    pub fn config(&self) -> &ResolvedPrintConfig {
        &self.config
    }

    /// Return the machine program the file will carry: the per-layer
    /// timings and the print time, volume and mass it reports.
    pub fn program(&self) -> &PrintProgram {
        &self.program
    }

    /// Write the job to `output` as a printer file of `format`.
    pub fn write(
        &self,
        output: &Path,
        format: PrinterFileFormat,
        previews: Option<&PrintPreviews>,
    ) -> Result<(), WriterError> {
        write_printer_file(
            self.contours.stack(),
            output,
            format,
            &self.program,
            previews,
        )
    }
}

/// Why a slice cannot be exported as a printer file.
#[derive(Clone, Debug, PartialEq)]
pub enum UnexportableSlice {
    /// The full print configuration does not resolve.
    Configuration(ResolveError),
    /// The configuration resolves a layer height other than the one the
    /// contours were cut at.
    LayerHeightMismatch {
        /// The layer height the contours were cut at.
        sliced: PositiveFiniteMm,
        /// The layer height the configuration resolves.
        resolved: PositiveFiniteMm,
    },
    /// The first layer's height is not on the printer's Z step.
    FirstLayerOffZStep {
        /// The first layer's height.
        first_layer: PositiveFiniteMm,
        /// The nearest height on the printer's Z step.
        nearest: PositiveFiniteMm,
    },
    /// The print program cannot be built.
    Program(UnprojectablePrint),
}

impl fmt::Display for UnexportableSlice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(err) => {
                write!(f, "could not resolve the print configuration: {err}")
            }
            Self::LayerHeightMismatch { sliced, resolved } => write!(
                f,
                "the print configuration resolves {resolved} mm layers for contours cut at {sliced} mm",
            ),
            Self::FirstLayerOffZStep {
                first_layer,
                nearest,
            } => write!(
                f,
                "the first layer is {first_layer} mm high, off the printer's Z step; the nearest height on the step is {nearest} mm",
            ),
            Self::Program(err) => write!(f, "cannot describe this print to the machine: {err}"),
        }
    }
}

impl std::error::Error for UnexportableSlice {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Configuration(err) => Some(err),
            Self::Program(err) => Some(err),
            Self::LayerHeightMismatch { .. } | Self::FirstLayerOffZStep { .. } => None,
        }
    }
}

/// Measure the resin `contours` cure on `printer` with `aa_level`, under the
/// raster an export of them rasterises with, and keep it with them: a later
/// [`prepare_export`] of the same contours reads it at once (#115, #434).
/// Returns the volume, in cubic millimetres.
pub fn measure_cured_resin(
    contours: &PlateContours,
    printer: &PrinterProfile,
    aa_level: AaLevel,
) -> f64 {
    let LayerSchedule::Uniform(grid) = *contours.schedule();
    let profile = project_hardware_to_slice_profile(
        &printer.hardware,
        grid.layer_height(),
        aa_level,
        PackingMode::None,
        grid.first_layer_height(),
        grid.total_layers(),
    );
    contours.lit_volume_mm3(&RasterSetup::from_profile(&profile))
}

/// Prepare the printer-file export of `contours` for the chosen printer and
/// material. The resin it reports is what the layers light (#115, #434),
/// measured once per slice and raster ([`PlateContours::lit_volume_mm3`]).
///
/// `overrides` holds the per-print values the contours were sliced with, such
/// as a requested layer height, and `aa_level` the anti-aliasing level to
/// rasterise with. A caller that holds a slice from an earlier run checks first
/// that the profiles and the scene still give the same contours.
///
/// The layers above the last one that lights a pixel are left out of the job
/// (#309): they print nothing, and a machine can stall on a dark last layer.
/// A plate that lights no pixel at all keeps its layers, which the writers
/// fill with a placeholder pixel each.
///
/// # Errors
///
/// [`UnexportableSlice`] when the full configuration does not resolve, when it
/// resolves another layer height than the contours were cut at, when the first
/// layer is off the printer's Z step, or when the print program cannot be
/// built.
pub fn prepare_export(
    contours: PlateContours,
    printer: &PrinterProfile,
    material: &EffectiveMaterial,
    overrides: &SceneOverrides,
    aa_level: AaLevel,
    slicer_version: &'static str,
) -> Result<ExportJob, UnexportableSlice> {
    let config = resolve(printer, material, overrides).map_err(UnexportableSlice::Configuration)?;
    let LayerSchedule::Uniform(grid) = *contours.schedule();
    let sliced = grid.layer_height();
    let resolved = config.material_resolved.layer_height_mm;
    if sliced != resolved {
        return Err(UnexportableSlice::LayerHeightMismatch { sliced, resolved });
    }
    let first_layer = grid.first_layer_height();
    let nearest = snap_to_z_step(&printer.constraints, first_layer);
    if nearest != first_layer {
        return Err(UnexportableSlice::FirstLayerOffZStep {
            first_layer,
            nearest,
        });
    }
    let profile_for = |layers: NonZeroU32| {
        project_hardware_to_slice_profile(
            &printer.hardware,
            sliced,
            aa_level,
            PackingMode::None,
            first_layer,
            layers,
        )
    };
    let mut profile = profile_for(grid.total_layers());
    let lit = lit_layer_count(contours.stack(), &RasterSetup::from_profile(&profile));
    let contours = match u32::try_from(lit).ok().and_then(NonZeroU32::new) {
        Some(lit) if lit < grid.total_layers() => {
            profile = profile_for(lit);
            contours.truncated(lit)
        }
        _ => contours,
    };
    let volume_mm3 = contours.lit_volume_mm3(&RasterSetup::from_profile(&profile));
    let program = project_to_print_program(
        volume_mm3,
        &config,
        &profile,
        creation_time_now(),
        slicer_version,
    )
    .map_err(UnexportableSlice::Program)?;
    Ok(ExportJob {
        contours,
        config,
        program,
    })
}
