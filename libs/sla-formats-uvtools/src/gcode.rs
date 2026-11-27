/*
 * SPDX-FileCopyrightText: 2020-2026 PTRTECH, Tiago Conceição (sn4k3) and UVtools contributors
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-or-later
 *
 * Ported to Rust from UVtools UVtools.Core/GCode/GCodeBuilder.cs and
 * GCodeCommand.cs, UVtools v7.1.1, commit
 * 8b5c91fd41b60bd6265ba1c1f2b2b64636b873e8, modified for Breadslicer: only
 * the writing half is ported, the events become a trait, the layers come
 * from a Breadslicer print program, and the first comment names Breadslicer.
 *
 * This program is free software: you can redistribute it and/or modify it
 * under the terms of the GNU Affero General Public License as published by the
 * Free Software Foundation, either version 3 of the License, or (at your
 * option) any later version. It is distributed WITHOUT ANY WARRANTY; see the
 * GNU Affero General Public License (LICENSE.md) for details.
 */

//! The G-code the archive formats carry beside their layer images.
//!
//! Each format that runs its machine from G-code (the ChiTuBox and Klipper
//! ZIPs, NovaMaker CWS, NanoDLP, ...) writes the same program: a start block,
//! one block per layer (show the image, lift, retract, wait, expose) and an
//! end block that raises the platform. They differ in the commands they use,
//! the units and the extra lines around each block: a [`Dialect`] holds the
//! commands and units, and [`Hooks`] add the lines.

use sc_sla_formats::PrintProgram;

use crate::program::{StageSettings, round_away, round_height};

/// Whether moves state where to go or how far.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Positioning {
    Absolute,
    Relative,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimeUnit {
    Milliseconds,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpeedUnit {
    MillimetersPerMinute,
}

/// How a layer's image is named in the show-image command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShowImage {
    /// `0.png`, `1.png`, ...
    FilenamePng0,
    /// `1.png`, `2.png`, ...
    FilenamePng1,
    /// `0`, `1`, ...
    Index0,
}

/// Where in a layer's block its image is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShowImageAt {
    /// First, before the platform moves.
    FirstLine,
    /// Just before the light comes on.
    WhenRequired,
}

/// Which of the moves uses `G0` (rapid) and which `G1`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MoveCommand {
    G0,
    G1,
}

/// One command: its name, its arguments as a template (`{0}`, `{1}` stand
/// for the values), the comment it carries, and whether it is written at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Command {
    pub name: String,
    pub arguments: Option<String>,
    pub comment: Option<String>,
    pub enabled: bool,
}

impl Command {
    fn new(name: &str, arguments: Option<&str>, comment: &str, enabled: bool) -> Self {
        Self {
            name: name.to_owned(),
            arguments: arguments.map(str::to_owned),
            comment: Some(comment.to_owned()),
            enabled,
        }
    }

    /// Changes what the command writes, keeping its comment.
    pub(crate) fn set(&mut self, name: &str, arguments: Option<&str>) {
        self.name = name.to_owned();
        self.arguments = arguments.map(str::to_owned);
    }

    /// The line, with `values` in place of the template's slots.
    fn line(
        &self,
        comments: bool,
        tail_comma: bool,
        comment: Option<&str>,
        values: &[&str],
    ) -> String {
        let mut line = self.name.clone();
        if let Some(arguments) = self.arguments.as_deref().filter(|a| !a.trim().is_empty()) {
            line.push(' ');
            line.push_str(arguments);
        }
        if !(line.is_empty() || line.starts_with(';')) {
            let comment = comment
                .filter(|c| !c.trim().is_empty())
                .or(self.comment.as_deref());
            match comment.filter(|c| !c.trim().is_empty()) {
                Some(comment) if comments => {
                    line.push(';');
                    line.push_str(comment);
                }
                _ if tail_comma => line.push(';'),
                _ => {}
            }
        }
        fill(&line, values)
    }
}

/// Puts `values[n]` where `{n}` is.
fn fill(template: &str, values: &[&str]) -> String {
    let mut out = template.to_owned();
    for (index, value) in values.iter().enumerate() {
        out = out.replace(&format!("{{{index}}}"), value);
    }
    out
}

/// A float as .NET's invariant culture writes it: the shortest text that
/// reads back to it, no trailing `.0`.
pub(crate) fn num(value: f32) -> String {
    let text = format!("{value}");
    text.strip_suffix(".0").map(str::to_owned).unwrap_or(text)
}

/// What a format's G-code uses.
#[derive(Clone, Debug)]
pub(crate) struct Dialect {
    pub positioning: Positioning,
    pub time_unit: TimeUnit,
    pub speed_unit: SpeedUnit,
    pub show_image: ShowImage,
    pub show_image_at: ShowImageAt,
    pub clear_image_at_start: bool,
    pub clear_image_per_layer: bool,
    pub clear_image_at_end: bool,
    pub layer_move: MoveCommand,
    pub end_move: MoveCommand,
    pub tail_comma: bool,
    pub comments: bool,
    /// The light's full power: a layer's 0..=255 duty is scaled to it.
    pub max_led_power: u16,
    /// Whether the previews go in the G-code, as base64 comments.
    pub encode_thumbnails: bool,

    pub begin_start_comments: String,
    pub end_start_comments: String,
    /// `{0}` is the layer index, `{1}` its height.
    pub begin_layer_comments: String,
    pub end_layer_comments: String,
    pub begin_end_comments: String,
    pub end_end_comments: String,

    pub units_mm_g21: Command,
    pub absolute_g90: Command,
    pub relative_g91: Command,
    pub motors_on_m17: Command,
    pub motors_off_m18: Command,
    pub home_g28: Command,
    pub move_g0: Command,
    pub move_g1: Command,
    pub acceleration_m204: Command,
    pub sync_movements: Command,
    pub wait_sync_delay: Command,
    pub wait_g4: Command,
    pub show_image_m6054: Command,
    pub clear_image: Command,
    pub turn_led_m106: Command,
    pub turn_led_off: Command,
    pub print_progress_m73: Command,
    pub klipper_turn_led_m1400: Command,
}

impl Default for Dialect {
    fn default() -> Self {
        Self {
            positioning: Positioning::Absolute,
            time_unit: TimeUnit::Milliseconds,
            speed_unit: SpeedUnit::MillimetersPerMinute,
            show_image: ShowImage::FilenamePng1,
            show_image_at: ShowImageAt::FirstLine,
            clear_image_at_start: true,
            clear_image_per_layer: true,
            clear_image_at_end: true,
            layer_move: MoveCommand::G0,
            end_move: MoveCommand::G0,
            tail_comma: true,
            comments: true,
            max_led_power: 255,
            encode_thumbnails: false,

            begin_start_comments: ";START_GCODE_BEGIN".to_owned(),
            end_start_comments: ";END_GCODE_BEGIN".to_owned(),
            begin_layer_comments: ";LAYER_START:{0}\n;PositionZ:{1}mm".to_owned(),
            end_layer_comments: ";LAYER_END".to_owned(),
            begin_end_comments: ";START_GCODE_END".to_owned(),
            end_end_comments: ";END_GCODE_END\n;<Completed>".to_owned(),

            units_mm_g21: Command::new("G21", None, "Set units to be mm", true),
            absolute_g90: Command::new("G90", None, "Absolute positioning", true),
            relative_g91: Command::new("G91", None, "Partial positioning", true),
            motors_on_m17: Command::new("M17", None, "Enable motors", true),
            motors_off_m18: Command::new("M18", None, "Disable motors", true),
            home_g28: Command::new("G28", Some("Z0"), "Home Z", true),
            move_g0: Command::new("G0", Some("Z{0} F{1}"), "Move Z", true),
            move_g1: Command::new("G1", Some("Z{0} F{1}"), "Move Z", true),
            acceleration_m204: Command::new("M204", Some("S{0}"), "Set acceleration", false),
            sync_movements: Command::new("G4", Some("P0"), "Sync movements", false),
            wait_sync_delay: Command::new("G4", Some("P0{0}"), "Sync movement", false),
            wait_g4: Command::new("G4", Some("P{0}"), "Delay", true),
            show_image_m6054: Command::new("M6054", Some("\"{0}\""), "Show image", true),
            clear_image: Command {
                name: ";<Slice> Blank".to_owned(),
                arguments: None,
                comment: None,
                enabled: true,
            },
            turn_led_m106: Command::new("M106", Some("S{0}"), "Turn LED", true),
            turn_led_off: Command::new("M106", Some("S0"), "Turn LED off", true),
            print_progress_m73: Command::new("M73", Some("P{0} R{1}"), "Set print progress", false),
            klipper_turn_led_m1400: Command::new("M1400", Some("S{0} P{1}"), "Turn LED", false),
        }
    }
}

impl Dialect {
    /// Switches to the commands Klipper's resin firmware reads.
    pub(crate) fn klipper(mut self) -> Self {
        self.clear_image_at_start = false;
        self.clear_image_per_layer = false;
        self.clear_image_at_end = true;
        self.motors_on_m17.enabled = false;
        self.home_g28.arguments = Some("Z".to_owned());
        self.sync_movements.set("M400", None);
        self.wait_sync_delay.set("M400", None);
        self.turn_led_m106.enabled = false;
        self.clear_image.name = "M1450".to_owned();
        self.show_image_m6054.name = "M1451".to_owned();
        if let Some(arguments) = self.show_image_m6054.arguments.as_mut()
            && !arguments.starts_with('F')
        {
            *arguments = format!("F{arguments}");
        }
        self.print_progress_m73.enabled = true;
        self.turn_led_off.set("M1401", None);
        self.klipper_turn_led_m1400.enabled = true;
        self.acceleration_m204.enabled = true;
        self.encode_thumbnails = true;
        self
    }

    fn time_in_unit(&self, seconds: f32) -> f32 {
        match self.time_unit {
            TimeUnit::Milliseconds => round_away(seconds * 1000.0, 2),
        }
    }

    fn to_seconds(&self, time: f32) -> f32 {
        match self.time_unit {
            TimeUnit::Milliseconds => round_away(time / 1000.0, 3),
        }
    }

    fn speed_in_unit(&self, mm_min: f32) -> f32 {
        match self.speed_unit {
            SpeedUnit::MillimetersPerMinute => mm_min,
        }
    }

    /// The name of layer `index`'s image in the show-image command.
    pub(crate) fn image_name(&self, index: usize) -> String {
        match self.show_image {
            ShowImage::FilenamePng0 => format!("{index}.png"),
            ShowImage::FilenamePng1 => format!("{}.png", index + 1),
            ShowImage::Index0 => index.to_string(),
        }
    }
}

/// One layer as the G-code drives it, in the units of the program.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct GLayer {
    pub position_z: f32,
    pub wait_before_cure_sec: f32,
    pub exposure_sec: f32,
    pub wait_after_cure_sec: f32,
    pub wait_after_lift_sec: f32,
    pub lift_mm: f32,
    pub lift_mm_min: f32,
    pub lift2_mm: f32,
    pub lift2_mm_min: f32,
    pub retract_mm: f32,
    pub retract_mm_min: f32,
    pub retract2_mm: f32,
    pub retract2_mm_min: f32,
    pub pwm: u8,
    /// How long the layer takes, for the progress the machine shows.
    pub print_time_sec: f32,
}

impl GLayer {
    pub(crate) fn lift_total_mm(&self) -> f32 {
        round_away(self.lift_mm + self.lift2_mm, 2)
    }
}

/// The program's layers as the G-code drives them.
///
/// A layer the platform does not lift on (a tilting vat) still moves up to its
/// height, at the speed of the nearest layer that lifts: the G-code has no
/// other way to get there.
pub(crate) fn layers_of(program: &PrintProgram) -> Vec<GLayer> {
    let fallback = program
        .layers
        .iter()
        .find_map(|layer| layer.lift.map(|stage| stage.speed_mm_min.value() as f32))
        .unwrap_or(60.0);
    program
        .layers
        .iter()
        .map(|layer| {
            let stage = StageSettings::of(Some(layer));
            let (lift2_mm, lift2_mm_min) = layer.second_lift.map_or((0.0, 0.0), |s| {
                (s.distance_mm.value() as f32, s.speed_mm_min.value() as f32)
            });
            let (retract2_mm, retract2_mm_min) = layer.second_retract.map_or((0.0, 0.0), |s| {
                (s.distance_mm.value() as f32, s.speed_mm_min.value() as f32)
            });
            let lift_mm_min = if stage.lift_mm_min > 0.0 {
                stage.lift_mm_min
            } else {
                fallback
            };
            let retract_mm_min = if stage.retract_mm_min > 0.0 {
                stage.retract_mm_min
            } else {
                lift_mm_min
            };
            // A layer keeps every value to 2 decimals, as UVtools' does.
            let r2 = |value: f32| round_away(value, 2);
            let mut glayer = GLayer {
                position_z: round_height(layer.slab_top_mm.value() as f32),
                wait_before_cure_sec: r2(stage.wait_before_cure_sec),
                exposure_sec: r2(stage.exposure_sec),
                wait_after_cure_sec: r2(stage.wait_after_cure_sec),
                wait_after_lift_sec: r2(stage.wait_after_lift_sec),
                lift_mm: r2(stage.lift_mm),
                lift_mm_min: r2(lift_mm_min),
                lift2_mm: r2(lift2_mm),
                lift2_mm_min: r2(lift2_mm_min),
                retract_mm: 0.0,
                retract_mm_min: r2(retract_mm_min),
                retract2_mm: r2(retract2_mm.min(stage.lift_mm + lift2_mm)),
                retract2_mm_min: r2(retract2_mm_min),
                pwm: stage.pwm,
                print_time_sec: 0.0,
            };
            // As UVtools: the first retract is what the second leaves.
            glayer.retract_mm = round_away(glayer.lift_total_mm() - glayer.retract2_mm, 2);
            glayer.print_time_sec = layer_time_sec(&glayer);
            glayer
        })
        .collect()
}

/// How long a layer takes: the waits, the exposure and the travel.
fn layer_time_sec(layer: &GLayer) -> f32 {
    let travel = |mm: f32, mm_min: f32| {
        if mm > 0.0 && mm_min > 0.0 {
            mm / (mm_min / 60.0)
        } else {
            0.0
        }
    };
    layer.wait_before_cure_sec
        + layer.exposure_sec
        + layer.wait_after_cure_sec
        + layer.wait_after_lift_sec
        + travel(layer.lift_mm, layer.lift_mm_min)
        + travel(layer.lift2_mm, layer.lift2_mm_min)
        + travel(layer.retract_mm, layer.retract_mm_min)
        + travel(layer.retract2_mm, layer.retract2_mm_min)
}

/// The time a lift takes, as UVtools sizes the waits that sync it.
fn lift_seconds(height: f32, speed: f32, height2: f32, speed2: f32, extra: f32) -> f32 {
    let mut time = extra;
    if height > 0.0 && speed > 0.0 {
        time += height / (speed / 60.0);
    }
    if height2 > 0.0 && speed2 > 0.0 {
        time += height2 / (speed2 / 60.0);
    }
    round_away(time, 2)
}

/// What the whole print is, for the end block and the hooks.
#[derive(Clone, Debug)]
pub(crate) struct Job<'a> {
    pub layers: &'a [GLayer],
    pub machine_z_mm: f32,
    /// The normal layers' first lift speed, for the end block.
    pub lift_mm_min: f32,
    /// The fastest the platform moves anywhere in the print.
    pub maximum_mm_min: f32,
    /// PNGs to embed when the dialect encodes thumbnails, with their sizes.
    pub thumbnails: Vec<(u32, u32, Vec<u8>)>,
}

impl<'a> Job<'a> {
    pub(crate) fn new(program: &PrintProgram, layers: &'a [GLayer]) -> Self {
        let first_normal = program
            .bottom_layer_count
            .saturating_add(program.transition_layer_count) as usize;
        let normal = layers.get(first_normal).or_else(|| layers.last());
        let bottom = layers.first();
        let speeds = |layer: Option<&GLayer>| {
            layer.map_or(0.0_f32, |l| {
                l.lift_mm_min
                    .max(l.lift2_mm_min)
                    .max(l.retract_mm_min)
                    .max(l.retract2_mm_min)
            })
        };
        Self {
            layers,
            machine_z_mm: program.raster.build_volume().z().value() as f32,
            lift_mm_min: normal.map_or(0.0, |l| l.lift_mm_min),
            maximum_mm_min: speeds(bottom).max(speeds(normal)),
            thumbnails: Vec::new(),
        }
    }
}

/// Lines a format adds around the blocks. Returning `true` from a `before_`
/// method replaces the default block.
pub(crate) trait Hooks {
    fn before_rebuild(&mut self, _g: &mut Builder) {}
    fn before_start(&mut self, _g: &mut Builder) -> bool {
        false
    }
    fn after_start(&mut self, _g: &mut Builder) {}
    fn before_layer(&mut self, _g: &mut Builder, _index: usize) -> bool {
        false
    }
    fn after_layer(&mut self, _g: &mut Builder, _index: usize) {}
    fn before_end(&mut self, _g: &mut Builder) -> bool {
        false
    }
    fn after_rebuild(&mut self, _g: &mut Builder) {}
}

/// No extra lines.
pub(crate) struct NoHooks;
impl Hooks for NoHooks {}

/// Writes G-code in a [`Dialect`].
#[derive(Debug)]
pub(crate) struct Builder {
    pub dialect: Dialect,
    out: String,
    last_acceleration: f32,
}

type Move = (f32, f32, f32);

impl Builder {
    pub(crate) fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            out: String::new(),
            last_acceleration: 0.0,
        }
    }

    pub(crate) fn text(&self) -> &str {
        &self.out
    }

    pub(crate) fn into_text(self) -> String {
        self.out
    }

    pub(crate) fn append(&mut self, text: &str) {
        self.out.push_str(text);
    }

    pub(crate) fn line(&mut self, text: &str) {
        self.out.push_str(text);
        self.out.push('\n');
    }

    pub(crate) fn blank(&mut self) {
        self.out.push('\n');
    }

    /// A comment line, `;` and `text`, when the dialect writes comments.
    pub(crate) fn comment(&mut self, text: &str) {
        if text.trim().is_empty() || !self.dialect.comments {
            return;
        }
        self.line(&format!(";{text}"));
    }

    fn line_if_comments(&mut self, text: &str) {
        if text.trim().is_empty() || !self.dialect.comments {
            return;
        }
        // A template may hold several lines.
        for part in text.split('\n') {
            self.line(part);
        }
    }

    fn command(&mut self, command: &Command, comment: Option<&str>, values: &[&str]) {
        if !command.enabled {
            return;
        }
        let text = command.line(
            self.dialect.comments,
            self.dialect.tail_comma,
            comment,
            values,
        );
        self.line(&text);
    }

    fn move_gx(&mut self, which: MoveCommand, z: f32, feed: f32, comment: Option<&str>) {
        if !z.is_finite() || !feed.is_finite() || z == 0.0 || feed <= 0.0 {
            return;
        }
        let command = match which {
            MoveCommand::G0 => self.dialect.move_g0.clone(),
            MoveCommand::G1 => self.dialect.move_g1.clone(),
        };
        self.command(&command, comment, &[&num(z), &num(feed)]);
    }

    fn wait_g4(&mut self, time: f32, comment: &str) {
        if !time.is_finite() || time < 0.0 {
            return;
        }
        let command = self.dialect.wait_g4.clone();
        self.command(&command, Some(comment), &[&num(time)]);
    }

    fn wait_sync_delay(&mut self, time: f32) {
        if !time.is_finite() || time < 0.0 {
            return;
        }
        let command = self.dialect.wait_sync_delay.clone();
        self.command(&command, None, &[&num(time)]);
    }

    fn sync_movements(&mut self) {
        let command = self.dialect.sync_movements.clone();
        self.command(&command, None, &[]);
    }

    fn clear_image(&mut self) {
        let command = self.dialect.clear_image.clone();
        self.command(&command, None, &[]);
    }

    fn print_progress(&mut self, percent: f32, minutes: f32) {
        if !percent.is_finite() || !minutes.is_finite() {
            return;
        }
        let command = self.dialect.print_progress_m73.clone();
        self.command(
            &command,
            None,
            &[&num(percent.clamp(0.0, 100.0)), &num(minutes.max(0.0))],
        );
    }

    fn turn_light(&mut self, value: u16, exposure: f32) {
        let value = value.min(self.dialect.max_led_power);
        if self.dialect.klipper_turn_led_m1400.enabled {
            if value == 0 {
                let name = self.dialect.klipper_turn_led_m1400.name.clone();
                self.line(&format!("{name} S0; Turn LED OFF"));
            } else {
                let seconds = round_away(self.dialect.to_seconds(exposure), 2);
                let comment = format!("Exposure for {}s", num(seconds));
                let command = self.dialect.klipper_turn_led_m1400.clone();
                self.command(
                    &command,
                    Some(&comment),
                    &[&value.to_string(), &num(exposure)],
                );
            }
        } else {
            let comment = if value == 0 {
                "Turn LED OFF"
            } else {
                "Turn LED ON"
            };
            let command = self.dialect.turn_led_m106.clone();
            self.command(&command, Some(comment), &[&value.to_string()]);
        }
    }

    fn light_off(&mut self) {
        self.turn_light(0, 0.0);
    }

    fn exposure(&mut self, time: f32, pwm: u16) {
        if !time.is_finite() || pwm == 0 || self.dialect.max_led_power == 0 || time <= 0.0 {
            return;
        }
        self.turn_light(pwm, time);
        if !self.dialect.klipper_turn_led_m1400.enabled {
            self.wait_g4(time, "Cure time/delay");
            self.light_off();
        }
        if self.dialect.clear_image_per_layer {
            self.clear_image();
        }
    }

    fn lift_move(
        &mut self,
        which: MoveCommand,
        lifts: &[Move],
        retracts: &[Move],
        wait_after_lift: f32,
        layer: Option<&GLayer>,
    ) {
        let valid_lift = |m: &Move| m.0 > 0.0 && m.1 > 0.0 && m.0.is_finite() && m.1.is_finite();
        let valid_retract =
            |m: &Move| m.0 != 0.0 && m.1 > 0.0 && m.0.is_finite() && m.1.is_finite();
        if !lifts.iter().any(valid_lift) {
            return;
        }
        let (name, alias) = match which {
            MoveCommand::G0 => (self.dialect.move_g0.clone(), "Z Lift"),
            MoveCommand::G1 => (self.dialect.move_g1.clone(), "Z Lift"),
        };
        for (i, lift) in lifts.iter().enumerate() {
            if !valid_lift(lift) {
                continue;
            }
            self.acceleration(lift.2);
            let comment = format!("{alias} ({})", i + 1);
            self.command(&name, Some(&comment), &[&num(lift.0), &num(lift.1)]);
        }
        if self.dialect.wait_sync_delay.enabled
            && let Some(layer) = layer
        {
            let seconds = lift_seconds(
                layer.lift_mm,
                layer.lift_mm_min,
                layer.lift2_mm,
                layer.lift2_mm_min,
                0.75,
            );
            let time = self.dialect.time_in_unit(seconds);
            self.wait_sync_delay(time);
        }
        self.sync_movements();
        if wait_after_lift > 0.0 {
            self.wait_g4(wait_after_lift, "Wait after lift");
        }
        if retracts.iter().any(valid_retract) {
            for (i, retract) in retracts.iter().enumerate() {
                if !valid_retract(retract) {
                    continue;
                }
                self.acceleration(retract.2);
                let comment = format!("Retract ({})", i + 1);
                self.command(&name, Some(&comment), &[&num(retract.0), &num(retract.1)]);
            }
            if self.dialect.wait_sync_delay.enabled
                && let Some(layer) = layer
            {
                let seconds = lift_seconds(
                    layer.retract_mm,
                    layer.retract_mm_min,
                    layer.retract2_mm,
                    layer.retract2_mm_min,
                    0.75,
                );
                let time = self.dialect.time_in_unit(seconds);
                self.wait_sync_delay(time);
            }
            self.sync_movements();
        }
    }

    fn acceleration(&mut self, acceleration: f32) {
        if self.dialect.acceleration_m204.enabled
            && acceleration > 0.0
            && acceleration != self.last_acceleration
        {
            let command = self.dialect.acceleration_m204.clone();
            self.command(&command, None, &[&num(acceleration)]);
            self.last_acceleration = acceleration;
        }
    }

    fn positioning(&mut self) {
        let command = match self.dialect.positioning {
            Positioning::Absolute => self.dialect.absolute_g90.clone(),
            Positioning::Relative => self.dialect.relative_g91.clone(),
        };
        self.command(&command, None, &[]);
    }

    fn start_block(&mut self, hooks: &mut dyn Hooks) {
        let begin = self.dialect.begin_start_comments.clone();
        self.line_if_comments(&begin);
        if !hooks.before_start(self) {
            let units = self.dialect.units_mm_g21.clone();
            self.command(&units, None, &[]);
            self.positioning();
            self.light_off();
            if self.dialect.clear_image_at_start {
                self.clear_image();
            }
            let on = self.dialect.motors_on_m17.clone();
            self.command(&on, None, &[]);
            let home = self.dialect.home_g28.clone();
            self.command(&home, None, &[]);
            self.sync_movements();
        }
        hooks.after_start(self);
        let end = self.dialect.end_start_comments.clone();
        self.line_if_comments(&end);
        self.blank();
    }

    fn end_block(&mut self, job: &Job, hooks: &mut dyn Hooks) {
        let begin = self.dialect.begin_end_comments.clone();
        self.line_if_comments(&begin);
        if !hooks.before_end(self) {
            self.print_progress(100.0, 0.0);
            self.light_off();
            if self.dialect.clear_image_at_end {
                self.clear_image();
            }
            if let Some(last) = job.layers.last()
                && last.position_z < job.machine_z_mm
            {
                let last_lift = last.lift_mm.max(4.0);
                let last_position = round_height(last.position_z + last_lift).min(job.machine_z_mm);
                let lift_speed = self.dialect.speed_in_unit(job.lift_mm_min);
                let lift = match self.dialect.positioning {
                    Positioning::Absolute => (last_position, lift_speed, 0.0),
                    Positioning::Relative => (last_lift, lift_speed, 0.0),
                };
                let which = self.dialect.layer_move;
                self.lift_move(which, &[lift], &[], 0.0, Some(last));
                if last_position < job.machine_z_mm {
                    let relative = round_height(job.machine_z_mm - last_position);
                    let target = match self.dialect.positioning {
                        Positioning::Relative => relative,
                        Positioning::Absolute => job.machine_z_mm,
                    };
                    if relative > 0.0 {
                        let feed = self.dialect.speed_in_unit(job.maximum_mm_min);
                        let end_move = self.dialect.end_move;
                        self.move_gx(end_move, target, feed, None);
                        if self.dialect.wait_sync_delay.enabled {
                            let seconds = lift_seconds(
                                relative + last_lift,
                                job.maximum_mm_min,
                                0.0,
                                0.0,
                                0.75,
                            );
                            let time = self.dialect.time_in_unit(seconds);
                            if seconds > 0.0 {
                                self.wait_sync_delay(time);
                            }
                        }
                        self.sync_movements();
                    }
                }
            }
            let off = self.dialect.motors_off_m18.clone();
            self.command(&off, None, &[]);
        }
        let end = self.dialect.end_end_comments.clone();
        self.line_if_comments(&end);
    }

    /// Writes the whole program: `header` (the format's settings block, may be
    /// empty) after the thumbnails, then the start, layer and end blocks.
    pub(crate) fn rebuild(&mut self, job: &Job, header: &str, hooks: &mut dyn Hooks) {
        self.out.clear();
        self.last_acceleration = 0.0;
        hooks.before_rebuild(self);
        self.comment(&format!(
            " Generated by Breadslicer {}",
            env!("CARGO_PKG_VERSION")
        ));

        if self.dialect.encode_thumbnails && !job.thumbnails.is_empty() {
            self.blank();
            for (width, height, png) in &job.thumbnails {
                let base64 = base64(png);
                self.blank();
                self.line(";");
                self.line(&format!(
                    "; thumbnail begin {width}x{height} {}",
                    base64.len()
                ));
                for chunk in base64.as_bytes().chunks(78) {
                    self.line(&format!("; {}", std::str::from_utf8(chunk).expect("ascii")));
                }
                self.line("; thumbnail end");
                self.line(";");
            }
            self.blank();
            self.blank();
        }

        if job.layers.is_empty() {
            hooks.after_rebuild(self);
            return;
        }
        if !header.trim().is_empty() {
            self.append(header);
            self.blank();
        }
        self.start_block(hooks);

        let mut last_z = 0.0_f32;
        let mut remaining: f32 = job.layers.iter().map(|l| l.print_time_sec).sum();
        let count = job.layers.len();
        for (index, layer) in job.layers.iter().enumerate() {
            let d = &self.dialect;
            let wait_before = d.time_in_unit(layer.wait_before_cure_sec);
            let exposure = d.time_in_unit(layer.exposure_sec);
            let wait_after_cure = d.time_in_unit(layer.wait_after_cure_sec);
            let wait_after_lift = d.time_in_unit(layer.wait_after_lift_sec);
            let lift_speed = d.speed_in_unit(layer.lift_mm_min);
            let lift2_speed = d.speed_in_unit(layer.lift2_mm_min);
            let retract_speed = d.speed_in_unit(layer.retract_mm_min);
            let retract2_speed = d.speed_in_unit(layer.retract2_mm_min);
            let lift_total = layer.lift_total_mm();
            let mut pwm = u16::from(layer.pwm);
            if d.max_led_power != 255 {
                pwm = d.max_led_power * pwm / 255;
            }

            let mut lifts: Vec<Move> = Vec::with_capacity(2);
            let mut retracts: Vec<Move> = Vec::with_capacity(2);
            match d.positioning {
                Positioning::Absolute => {
                    let up = round_height(layer.lift_mm + layer.position_z);
                    let up2 = round_height(up + layer.lift2_mm);
                    let down = round_height(up2 - layer.retract_mm);
                    if layer.lift_mm > 0.0 {
                        lifts.push((up, lift_speed, 0.0));
                    }
                    if layer.lift2_mm > 0.0 {
                        lifts.push((up2, lift2_speed, 0.0));
                    }
                    if layer.retract_mm > 0.0 && down >= layer.position_z {
                        retracts.push((down, retract_speed, 0.0));
                    }
                    if layer.retract2_mm > 0.0 && down > layer.position_z {
                        retracts.push((layer.position_z, retract2_speed, 0.0));
                    }
                }
                Positioning::Relative => {
                    let partial = round_height(layer.position_z - last_z + layer.lift_mm);
                    if layer.lift_mm > 0.0 {
                        lifts.push((partial, lift_speed, 0.0));
                        if layer.lift2_mm > 0.0 {
                            lifts.push((layer.lift2_mm, lift2_speed, 0.0));
                        }
                    } else if layer.lift2_mm > 0.0 {
                        lifts.push((round_height(partial + layer.lift2_mm), lift2_speed, 0.0));
                    }
                    if round_height(layer.retract_mm + layer.retract2_mm) != lift_total {
                        retracts.push((-lift_total, retract_speed, 0.0));
                    } else {
                        if layer.retract_mm > 0.0 {
                            retracts.push((-layer.retract_mm, retract_speed, 0.0));
                        }
                        if layer.retract2_mm > 0.0 {
                            retracts.push((-layer.retract2_mm, retract2_speed, 0.0));
                        }
                    }
                }
            }

            let begin = fill(
                &self.dialect.begin_layer_comments,
                &[&index.to_string(), &num(layer.position_z)],
            );
            self.line_if_comments(&begin);
            if !hooks.before_layer(self, index) {
                self.print_progress(
                    round_away(index as f32 * 100.0 / count as f32, 2),
                    round_away(remaining / 60.0, 2),
                );
                let image = self.dialect.image_name(index);
                let show = self.dialect.show_image_m6054.clone();
                if self.dialect.show_image_at == ShowImageAt::FirstLine {
                    self.command(&show, None, &[&image]);
                }
                if lift_total > 0.0
                    && round_height(lift_total + layer.position_z) > layer.position_z
                {
                    let which = self.dialect.layer_move;
                    self.lift_move(which, &lifts, &retracts, wait_after_lift, Some(layer));
                } else if last_z != layer.position_z {
                    let speed = if last_z < layer.position_z {
                        lift_speed
                    } else {
                        retract_speed
                    };
                    let target = match self.dialect.positioning {
                        Positioning::Absolute => layer.position_z,
                        Positioning::Relative => round_height(layer.position_z - last_z),
                    };
                    let which = self.dialect.layer_move;
                    self.move_gx(which, target, speed, None);
                }
                self.wait_g4(wait_before, "Wait before cure");
                if self.dialect.show_image_at == ShowImageAt::WhenRequired && exposure > 0.0 {
                    self.command(&show, None, &[&image]);
                }
                self.exposure(exposure, pwm);
                if wait_after_cure > 0.0 {
                    self.wait_g4(wait_after_cure, "Wait after cure");
                }
            }
            hooks.after_layer(self, index);
            let end = self.dialect.end_layer_comments.clone();
            self.line_if_comments(&end);
            self.blank();

            last_z = layer.position_z;
            remaining -= layer.print_time_sec;
        }

        self.end_block(job, hooks);
        hooks.after_rebuild(self);
    }
}

/// Standard base64 with padding.
pub(crate) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests_gcode;
