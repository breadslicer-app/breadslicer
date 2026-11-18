/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Liqcreate's resin settings, one page per printer, as material profiles
//! (#176).
//!
//! Liqcreate publishes its resins' settings for many printers, not only its
//! own: one support article per printer (`liqcreate.com/supportarticles/
//! <page>/`), with a table of a row per resin and layer thickness. The site
//! refuses plain downloads, so the inputs are the pages saved as Markdown,
//! each named after its page (`<page>.md`). The tables' columns change from
//! page to page; each is known by its heading. Pages for printers we do not
//! ship, tables that give no exposure (the pages of professional machines
//! that state a curing time or a profile file to use), and rows without the
//! lift and its speeds (a printer's speed mode named instead) are noted.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use sc_print_primitives::{
    MaterialId, NonEmptyString, NonNegativeFiniteSec, PositiveFiniteMm, PositiveFiniteMmPerMin,
    PositiveFiniteSec, PrinterId,
};
use sc_print_profiles::{
    CalibrationTarget, MaterialDefaults, MaterialIdentity, MaterialIntent, MaterialPerPrinter,
    MaterialPhysical, MaterialProfile,
};
use semver::Version;

use crate::elegoo_settings::{microns, slug};
use crate::profile::ConvertedProfile;
use crate::reader::{ProfileReader, SourceReading};
use crate::source_note::SourceNote;

/// Each page, against the printer we ship it is for. A page whose printer
/// we only nearly ship (another variant, another screen) is left out.
const PAGES: &[(&str, &str)] = &[
    ("3d-printing-parameters-elegoo-mars-3", "elegoo-mars-3"),
    ("3d-printing-parameters-elegoo-mars-4", "elegoo-mars-4"),
    ("anycubic-photon-m3-parameters", "anycubic-photon-m3"),
    ("anycubic-photon-m3-plus-resin", "anycubic-photon-mono-x-6k-m3-plus"),
    ("anycubic-photon-m3-premium-resin", "anycubic-photon-m3-premium"),
    ("anycubic-photon-mono4k-settings-resin", "anycubic-photon-mono-4k"),
    ("anycubic-photon-p1-max-resin-settings", "anycubic-photon-p1-max"),
    ("anycubic-photon-p1-resin", "anycubic-photon-p1"),
    ("anycubic-photon-ultra-d2-settings-resin", "anycubic-photon-d2"),
    ("anycubic-photon-ultra-dlp-settings", "anycubic-photon-ultra"),
    ("creality-halot-mage-8k-resin", "creality-halot-mage"),
    ("creality-halot-mage-pro-8k-resin", "creality-halot-mage-pro"),
    ("creality-halot-mage-s-14k", "creality-halot-mage-s"),
    ("creality-halot-play-resin", "creality-halot-play"),
    ("creality-halot-r6-resin", "creality-halot-r6"),
    ("creality-ld006-resin-settings", "creality-ld-006"),
    ("elegoo-jupiter2-resin-settings", "elegoo-jupiter-2"),
    ("elegoo-mars-pro-resin-settings", "elegoo-mars-pro"),
    ("elegoo-mars5-resin-3dprinting", "elegoo-mars-5"),
    ("elegoo-mars5-ultra-resin-3dprinting", "elegoo-mars-5-ultra"),
    ("elegoo-saturn-8k-resin-parameters", "elegoo-saturn-8k"),
    ("elegoo-saturn4-ultra-resin", "elegoo-saturn-4-ultra"),
    ("elegoo-saturn4-ultra-resin-16k", "elegoo-saturn-4-ultra-16k"),
    ("mars-4-ultra-9k-lcd-resin-settings", "elegoo-mars-4-ultra"),
    ("mars4-dlp-resin-settings", "elegoo-mars-4-dlp"),
    ("nova3d-bene6-resin", "nova3d-bene6"),
    ("orange-longer-4k-settings", "longer-orange-4k"),
    ("parameters-phrozen-sonic-mega-8k", "phrozen-sonic-mega-8k"),
    ("parameters-phrozen-sonic-mega-8ks", "phrozen-sonic-mega-8k-s"),
    ("parameters-resin-anycubic-monosq", "anycubic-photon-mono-sq"),
    ("parameters-resin-anycubic-photon", "anycubic-photon"),
    ("parameters-resin-elegoo-saturn3-12k-ultra", "elegoo-saturn-3-ultra"),
    ("photon-mono2-resin-settings-4k", "anycubic-photon-mono-2"),
    ("phrozen-shuffle-2018-resin-settings", "phrozen-shuffle"),
    ("phrozen-shuffle-xl-resin", "phrozen-shuffle-xl"),
    ("phrozen-sonic-mighty-revo-14k-resin", "phrozen-sonic-mighty-revo"),
    ("phrozen-sonic-mighty-revo-16k-resin", "phrozen-sonic-mighty-revo-16k"),
    ("phrozen-sonic-mini-8ks-resin-3dprinter", "phrozen-sonic-mini-8k-s"),
    ("phrozen-transform-resin-settings", "phrozen-transform"),
    ("prusa-sl1s-speed-resin-settings", "prusa-sl1s-speed"),
    ("resin-3d-printing-settings-creality-halot-max", "creality-halot-max"),
    ("resin-anycubic-mono-m5s-pro-14k", "anycubic-photon-mono-m5s-pro"),
    ("resin-anycubic-mono-m7-14k", "anycubic-photon-mono-m7"),
    ("resin-anycubic-mono-m7-max", "anycubic-photon-mono-m7-max"),
    ("resin-anycubic-photon-mono4", "anycubic-photon-mono-4"),
    ("resin-anycubic-photon-mono4-ultra", "anycubic-photon-mono-4-ultra"),
    ("resin-compatible-creality-ld002h", "creality-ld-002h"),
    ("resin-elegoo-mars4-max", "elegoo-mars-4-max"),
    ("resin-parameters-anycubic-photon-mono", "anycubic-photon-mono"),
    ("resin-parameters-anycubic-photon-mono-se", "anycubic-photon-mono-se"),
    ("resin-parameters-elegoo-jupiter-se", "elegoo-jupiter-se"),
    ("resin-parameters-elegoo-saturn-s", "elegoo-saturn-s"),
    ("resin-parameters-phrozen-sonic-mini8k", "phrozen-sonic-mini-8k"),
    ("resin-parameters-uniformation-gk3-pro", "uniformation-gk3-pro"),
    ("resin-phrozen-sonic-mega-8k-v2", "phrozen-sonic-mega-8k-v2"),
    ("resin-phrozen-sonic-xl-4k-2022", "phrozen-sonic-xl-4k-2022"),
    ("resin-settings-creality-halot-one", "creality-halot-one"),
    ("resin-settings-creality-halot-one-pro", "creality-halot-one-pro"),
    ("resin-settings-creality-halot-sky", "creality-halot-sky"),
    ("resin-settings-elegoo-mars2-pro", "elegoo-mars-2-pro"),
    ("resin-settings-elegoo-saturn", "elegoo-saturn"),
    ("resin-settings-epax-x1k-e6", "epax-x1k-2k-mono"),
    ("resin-settings-epax-x10", "epax-x10"),
    ("resin-settings-epax-x10-e10-mono", "epax-e10-mono"),
    ("resin-settings-flashforge-foto13-4k-mono", "flashforge-foto-13-3"),
    ("resin-settings-flashforge-foto6-0", "flashforge-foto-6-0"),
    ("resin-settings-kelant-s400", "kelant-s400"),
    ("resin-settings-peopoly-phenom-noir", "peopoly-phenom-noir"),
    ("resin-settings-peopoly-phenoml", "peopoly-phenom-l"),
    ("resin-settings-phrozen-sonic-mighty8k", "phrozen-sonic-mighty-8k"),
    ("resin-settings-prusa-sl1-parameters", "prusa-sl1"),
    ("resin-settings-qiditech-ibox-mono4k", "qidi-i-box-mono"),
    ("resin-settings-uniformation-gk3-ultra", "uniformation-gk3-ultra"),
    ("resin-settings-wanhao-duplicator-d7", "wanhao-d7"),
    ("resin-settings-zortrax-inkspire", "zortrax-inkspire"),
    ("resin-wanhao-cgr", "wanhao-cgr-mono"),
    ("resin-wanhao-duplicator-d8", "wanhao-d8"),
    ("settings-resin-anycubic-mono-m7-pro", "anycubic-photon-mono-m7-pro"),
    ("settings-resin-anycubic-mono-x2", "anycubic-photon-mono-x2"),
    ("settings-resin-anycubic-photon-monox", "anycubic-photon-mono-x"),
    ("settings-resin-anycubic-photon-monox6ks", "anycubic-photon-mono-x-6ks"),
    ("settings-resin-anycubic-photon-zero", "anycubic-photon-zero"),
    ("settings-resin-ceres-8-9-4k", "voxelab-ceres-8-9"),
    ("settings-resin-creality-ld002r", "creality-ld-002r"),
    ("settings-resin-elegoo-saturn3-12k", "elegoo-saturn-3"),
    ("settings-resin-elegoo-saturn4-12k", "elegoo-saturn-4"),
    ("uniformation-gktwo-resin-setting", "uniformation-gktwo"),
];

/// Reads Liqcreate's saved support pages of resin settings.
#[derive(Clone, Copy, Debug, Default)]
pub struct LiqcreateReader;

/// Everything that can stop this reader.
#[derive(Debug)]
pub enum LiqcreateError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidId {
        resin: String,
        source: sc_print_primitives::InvalidProfileId,
    },
}

impl fmt::Display for LiqcreateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::InvalidId { resin, source } => {
                write!(f, "resin {resin:?} makes no material id: {source}")
            }
        }
    }
}

impl std::error::Error for LiqcreateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::InvalidId { source, .. } => Some(source),
        }
    }
}

/// A calibration block with nothing set.
fn empty_block() -> MaterialPerPrinter {
    MaterialPerPrinter {
        normal_exposure_sec: None,
        bottom_exposure_sec: None,
        bottom_layer_count: None,
        wait_before_cure_sec: None,
        wait_after_cure_sec: None,
        wait_after_lift_sec: None,
        bottom_wait_before_cure_sec: None,
        bottom_wait_after_cure_sec: None,
        bottom_wait_after_lift_sec: None,
        lift_distance_mm: None,
        lift_speed_mm_min: None,
        retract_speed_mm_min: None,
        bottom_lift_distance_mm: None,
        bottom_lift_speed_mm_min: None,
        bottom_retract_speed_mm_min: None,
        light_pwm: None,
        bottom_light_pwm: None,
        second_travel: None,
        bottom_second_travel: None,
    }
}

/// A Markdown table cell as plain text: links to their text, emphasis and
/// footnote marks gone, `\*` and `**` alike.
fn plain(cell: &str) -> String {
    let mut text = cell.trim().to_owned();
    // [Text](url) -> Text
    while let (Some(open), Some(mid)) = (text.find('['), text.find("](")) {
        let Some(close) = text[mid..].find(')').map(|i| mid + i) else {
            break;
        };
        if open > mid {
            break;
        }
        text = format!("{}{}{}", &text[..open], &text[open + 1..mid], &text[close + 1..]);
    }
    text.replace("\\*", "")
        .replace('*', "")
        .replace('\\', "")
        .trim()
        .to_owned()
}

/// A cell's number: `7,0` or `7.0`; `None` for `n/a`, an empty cell or text.
fn number(cell: &str) -> Option<f64> {
    let text = plain(cell).replace(',', ".");
    let text = text.trim_end_matches(|c: char| c.is_alphabetic() || c == ' ' || c == 'µ');
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

/// A layer thickness, µm: `50 µm`, or `0,05 mm`.
fn thickness_um(cell: &str) -> Option<f64> {
    let value = number(cell).filter(|v| *v > 0.0)?;
    let mm = plain(cell).to_lowercase().contains("mm");
    Some(if mm || value < 1.0 { value * 1000.0 } else { value })
}

/// `90 / 120` as its 2 numbers.
fn pair(cell: &str) -> Option<(f64, f64)> {
    let text = plain(cell);
    let (a, b) = text.split_once('/')?;
    Some((number(a)?, number(b)?))
}

/// The Markdown tables of `text`: each a list of rows of cells, the
/// alignment row left out.
fn tables(text: &str) -> Vec<Vec<Vec<String>>> {
    let mut out: Vec<Vec<Vec<String>>> = Vec::new();
    let mut current: Vec<Vec<String>> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('|') {
            // One pipe off each end: an empty first cell (the resin of the
            // row above) stays a cell.
            let inner = line.strip_prefix('|').unwrap_or(line);
            let inner = inner.strip_suffix('|').unwrap_or(inner);
            let cells: Vec<String> = inner.split('|').map(|c| c.to_owned()).collect();
            if cells.iter().all(|c| c.trim().chars().all(|ch| ch == '-' || ch == ':')) {
                continue;
            }
            current.push(cells);
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// What a column of the table holds, by its heading.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Column {
    Layer,
    Exposure,
    BottomCount,
    BottomExposure,
    Lift,
    Speeds,
    Rest,
    LightOff,
    UvPower,
    Other,
}

fn column_of(heading: &str) -> Column {
    let h = plain(heading).to_lowercase();
    if h.contains("layer thickness") {
        Column::Layer
    } else if h.contains("base layer exposure") || h == "base exposure" {
        Column::BottomExposure
    } else if h.contains("base layer count")
        || h.contains("# of base layers")
        || h.contains("base layer /")
    {
        Column::BottomCount
    } else if h.contains("exposure time") || h.contains("normal exposure") {
        Column::Exposure
    } else if h.contains("lift height") {
        Column::Lift
    } else if h.contains("lift speed") {
        Column::Speeds
    } else if h.contains("rest time") || h.contains("wait time") {
        Column::Rest
    } else if h.contains("light off") || h.contains("off-time") || h.contains("off time") {
        Column::LightOff
    } else if h.contains("uv power") {
        Column::UvPower
    } else {
        Column::Other
    }
}

/// One table's resins: `(resin, layer µm, block)` for each row that gives
/// an exposure and a bottom exposure, and how many rows did not.
fn rows_of(table: &[Vec<String>]) -> Option<(Vec<(String, u32, MaterialPerPrinter)>, usize)> {
    let head = table.iter().position(|row| {
        row.iter()
            .any(|c| plain(c).to_lowercase().contains("layer thickness"))
    })?;
    let columns: Vec<Column> = table[head].iter().map(|c| column_of(c)).collect();
    if !columns.contains(&Column::Exposure) {
        return None;
    }
    let at = |want: Column| columns.iter().position(|c| *c == want);
    let mut out = Vec::new();
    let mut unread = 0;
    let mut resin = String::new();
    for row in &table[head + 1..] {
        let cell = |want: Column| at(want).and_then(|i| row.get(i)).map(String::as_str);
        // Data the page calls preliminary is not a validated setting.
        if row.iter().any(|c| c.to_lowercase().contains("prelimin")) {
            unread += 1;
            continue;
        }
        // A first cell in italics alone (`*Thick designs*`) qualifies the
        // resin above: the row is that resin's, the first at a thickness
        // standing.
        let first = row.first().map(|c| c.trim()).unwrap_or_default();
        let qualifier = first.len() > 2
            && first.starts_with('*')
            && first.ends_with('*')
            && !first.starts_with("**");
        let name = plain(first);
        if !name.is_empty() && !qualifier {
            resin = name;
        }
        if resin.is_empty() {
            continue;
        }
        let layer = cell(Column::Layer).and_then(thickness_um).filter(|v| *v >= 1.0);
        let exposure = cell(Column::Exposure).and_then(number).filter(|v| *v > 0.0);
        let bottom = cell(Column::BottomExposure).and_then(number).filter(|v| *v > 0.0);
        let (Some(layer), Some(exposure), Some(bottom)) = (layer, exposure, bottom) else {
            unread += 1;
            continue;
        };
        let mut block = empty_block();
        block.normal_exposure_sec = PositiveFiniteSec::new(exposure).ok();
        block.bottom_exposure_sec = PositiveFiniteSec::new(bottom).ok();
        // `2`, or `2 / 10` with the transition layers after.
        block.bottom_layer_count = cell(Column::BottomCount)
            .and_then(|c| number(c).or_else(|| pair(c).map(|(bottom, _)| bottom)))
            .filter(|n| *n >= 0.0 && n.fract() == 0.0)
            .map(|n| n as u32);
        if let Some(lift) = cell(Column::Lift)
            .and_then(number)
            .and_then(|v| PositiveFiniteMm::new(v).ok())
        {
            block.lift_distance_mm = Some(lift);
            block.bottom_lift_distance_mm = Some(lift);
        }
        if let Some((up, down)) = cell(Column::Speeds).and_then(pair) {
            let up = PositiveFiniteMmPerMin::new(up).ok();
            let down = PositiveFiniteMmPerMin::new(down).ok();
            block.lift_speed_mm_min = up;
            block.retract_speed_mm_min = down;
            block.bottom_lift_speed_mm_min = up;
            block.bottom_retract_speed_mm_min = down;
        }
        // A rest "in every step": after the lift, after the retract (before
        // the light) and after the light.
        // Or 3 rests, `2,0/0/0,5`: before the lift, after it, and after the
        // retract.
        let rests = cell(Column::Rest).map(|c| {
            plain(c)
                .split('/')
                .map(number)
                .collect::<Option<Vec<f64>>>()
        });
        if let Some(Some(three)) = &rests
            && let [before_lift, after_lift, after_retract] = three.as_slice()
        {
            let sec = |v: f64| NonNegativeFiniteSec::new(v).ok();
            block.wait_after_cure_sec = sec(*before_lift);
            block.wait_after_lift_sec = sec(*after_lift);
            block.wait_before_cure_sec = sec(*after_retract);
            block.bottom_wait_after_cure_sec = sec(*before_lift);
            block.bottom_wait_after_lift_sec = sec(*after_lift);
            block.bottom_wait_before_cure_sec = sec(*after_retract);
        } else if let Some(rest) = cell(Column::Rest)
            .and_then(number)
            .and_then(|v| NonNegativeFiniteSec::new(v).ok())
        {
            block.wait_before_cure_sec = Some(rest);
            block.wait_after_cure_sec = Some(rest);
            block.wait_after_lift_sec = Some(rest);
            block.bottom_wait_before_cure_sec = Some(rest);
            block.bottom_wait_after_cure_sec = Some(rest);
            block.bottom_wait_after_lift_sec = Some(rest);
        }
        // The light kept off before each layer, and before each bottom layer:
        // `2 / 2`, or one number for both.
        if let Some(cell) = cell(Column::LightOff) {
            let (layers, bottom) = pair(cell)
                .or_else(|| number(cell).map(|v| (v, v)))
                .unwrap_or((-1.0, -1.0));
            block.wait_before_cure_sec = NonNegativeFiniteSec::new(layers).ok();
            block.bottom_wait_before_cure_sec = NonNegativeFiniteSec::new(bottom).ok();
        }
        if let Some(power) = cell(Column::UvPower).and_then(number)
            && power > 0.0
            && power < 100.0
        {
            block.light_pwm = sc_print_primitives::LightPwm::new(power).ok();
            block.bottom_light_pwm = block.light_pwm;
        }
        // A block a slice can use states the travel too.
        if block.lift_distance_mm.is_none()
            || block.lift_speed_mm_min.is_none()
            || block.retract_speed_mm_min.is_none()
        {
            unread += 1;
            continue;
        }
        out.push((resin.clone(), layer.round() as u32, block));
    }
    Some((out, unread))
}

/// The library's family of a Liqcreate resin.
fn family_of(resin: &str) -> &'static str {
    let r = resin.to_lowercase();
    if r.contains("flex") || r.contains("elastomer") {
        "flexible"
    } else if r.contains("wax") || r.contains("cast") {
        "castable"
    } else if r.contains("bio-med") || r.contains("dental") || r.contains("gum") {
        "biocompatible"
    } else if r.contains("tough") || r.contains("strong") || r.contains("impact") || r.contains("hazard") {
        "engineering"
    } else {
        "standard"
    }
}

impl ProfileReader for LiqcreateReader {
    type Error = LiqcreateError;

    fn read(&self, inputs: &[&Path]) -> Result<SourceReading, Self::Error> {
        let mut files: Vec<PathBuf> = Vec::new();
        for path in inputs {
            if path.is_dir() {
                let entries = std::fs::read_dir(path).map_err(|source| LiqcreateError::Io {
                    path: path.to_path_buf(),
                    source,
                })?;
                for entry in entries.flatten() {
                    let file = entry.path();
                    if file.extension().is_some_and(|e| e == "md") {
                        files.push(file);
                    }
                }
            } else {
                files.push(path.to_path_buf());
            }
        }
        files.sort();
        let mut resins: BTreeMap<MaterialId, MaterialProfile> = BTreeMap::new();
        let mut notes = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).map_err(|source| LiqcreateError::Io {
                path: file.clone(),
                source,
            })?;
            let page = file
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let found: Vec<_> = tables(&text).iter().filter_map(|t| rows_of(t)).collect();
            let rows: usize = found.iter().map(|(rows, unread)| rows.len() + unread).sum();
            let Some(printer) = PAGES.iter().find(|(p, _)| *p == page).map(|(_, id)| *id) else {
                notes.push(SourceNote::RowsForAMachineWeDoNotShip {
                    model: page.clone(),
                    rows,
                });
                continue;
            };
            let printer = PrinterId::new(printer).expect("the map holds only ids we ship");
            let target = CalibrationTarget::from_printer(&printer);
            for (rows, unread) in found {
                if unread > 0 {
                    notes.push(SourceNote::RowsWeCannotRead {
                        model: page.clone(),
                        rows: unread,
                    });
                }
                for (resin, layer_um, block) in rows {
                    let id_text = format!("liqcreate-{}-{layer_um}um", slug(&resin));
                    let id = MaterialId::new(&id_text).map_err(|source| {
                        LiqcreateError::InvalidId {
                            resin: resin.clone(),
                            source,
                        }
                    })?;
                    let height = PositiveFiniteMm::new(f64::from(layer_um) / 1000.0).ok();
                    let profile = resins.entry(id.clone()).or_insert_with(|| MaterialProfile {
                        extension: false,
                        identity: MaterialIdentity {
                            id,
                            brand: NonEmptyString::new("Liqcreate").expect("not empty"),
                            family: NonEmptyString::new(family_of(&resin)).expect("a constant"),
                            name: NonEmptyString::new(&format!(
                                "{resin} ({} um)",
                                height.map(microns).unwrap_or(layer_um)
                            ))
                            .expect("not empty"),
                            profile_version: Version::new(1, 0, 0),
                            origin: None,
                        },
                        intent: MaterialIntent {
                            target_energy_density_mj_per_cm2: None,
                            target_layer_height_mm: height,
                        },
                        defaults: MaterialDefaults::default(),
                        physical: MaterialPhysical::default(),
                        per_printer: BTreeMap::new(),
                        notes: BTreeMap::new(),
                    });
                    // A page stating one resin twice at one thickness: the
                    // first row stands.
                    profile.per_printer.entry(target.clone()).or_insert(block);
                }
            }
        }
        Ok(SourceReading {
            profiles: resins
                .into_values()
                .map(ConvertedProfile::Material)
                .collect(),
            notes,
        })
    }
}

#[cfg(test)]
mod liqcreate_tests {
    use super::*;

    const PAGE: &str = "Some text.

## **Phrozen Sonic Mini 8K S resin parameters**

|||||||||
|---|---|---|---|---|---|---|---|
|**Resin**|**Layer thickness**|**Exposure time (seconds)**|**Base layer count**|**Base layer exposure**|**Lift Height**|**Lift speed / retract speed (mm/min)**|**Rest time in every step (seconds)**|
|[Deep Blue](https://www.liqcreate.com/product/deep-blue/)|100 µm|13,0|2|60|6,0|90 / 120|2,0|
||50 µm\\*|11,0|2|50|6,0|90 / 120|2,0|
|[Bio-Med Clear](https://www.liqcreate.com/product/bio-med-clear/)|100 µm|n/a||||||
|*Thick designs*|50 µm|9,0|2|40|6,0|90 / 120|2,0|
|**Clear**|**50 µm**|***preliminairy data***|5,0|2|40|8|2|

More text.
";

    fn read(pages: &[(&str, &str)]) -> SourceReading {
        let dir = tempfile::tempdir().unwrap();
        for (name, text) in pages {
            std::fs::write(dir.path().join(format!("{name}.md")), text).unwrap();
        }
        let path = dir.path().to_path_buf();
        LiqcreateReader.read(&[path.as_path()]).unwrap()
    }

    #[test]
    fn a_page_s_rows_become_resins_for_its_printer() {
        let reading = read(&[("phrozen-sonic-mini-8ks-resin-3dprinter", PAGE)]);
        let materials: Vec<&MaterialProfile> = reading
            .profiles
            .iter()
            .filter_map(|p| match p {
                ConvertedProfile::Material(m) => Some(m),
                _ => None,
            })
            .collect();
        assert_eq!(materials.len(), 3, "Deep Blue at 100 and 50 µm, Bio-Med Clear at 50");
        let fifty = materials
            .iter()
            .find(|m| m.identity.id.as_str() == "liqcreate-deep-blue-50um")
            .expect("the 50 µm row");
        assert_eq!(fifty.identity.brand.as_str(), "Liqcreate");
        let printer = PrinterId::new("phrozen-sonic-mini-8k-s").unwrap();
        let block = fifty.per_printer[&CalibrationTarget::from_printer(&printer)];
        assert_eq!(block.normal_exposure_sec.map(|s| s.value()), Some(11.0));
        assert_eq!(block.bottom_exposure_sec.map(|s| s.value()), Some(50.0));
        assert_eq!(block.bottom_layer_count, Some(2));
        assert_eq!(block.lift_distance_mm.map(|m| m.value()), Some(6.0));
        assert_eq!(block.lift_speed_mm_min.map(|m| m.value()), Some(90.0));
        assert_eq!(block.retract_speed_mm_min.map(|m| m.value()), Some(120.0));
        assert_eq!(block.wait_before_cure_sec.map(|s| s.value()), Some(2.0));
        // A qualifier row stays Bio-Med Clear's; the n/a and the
        // preliminary rows are noted, not made.
        assert!(
            materials
                .iter()
                .any(|m| m.identity.id.as_str() == "liqcreate-bio-med-clear-50um")
        );
        assert!(
            reading
                .notes
                .iter()
                .any(|n| matches!(n, SourceNote::RowsWeCannotRead { rows: 2, .. })),
            "{:?}",
            reading.notes
        );
    }

    #[test]
    fn transition_layers_three_rests_and_millimetres_are_read() {
        let page = "|||||||||
|---|---|---|---|---|---|---|---|
|**Resin**|**Layer thickness**|**Exposure time (seconds)**|**Base layer /transition layers**|**Base layer exposure**|**Lift Height**|**Lift speed / retract speed**|**Rest time per step (seconds)**|
|[Deep Blue](x)|0,05 mm|5,5|2 / 10|50|6|90 / 120|2,0/0/0,5|
";
        let reading = read(&[("elegoo-mars5-ultra-resin-3dprinting", page)]);
        let ConvertedProfile::Material(m) = &reading.profiles[0] else {
            panic!("a material");
        };
        assert_eq!(m.identity.id.as_str(), "liqcreate-deep-blue-50um");
        let printer = PrinterId::new("elegoo-mars-5-ultra").unwrap();
        let block = m.per_printer[&CalibrationTarget::from_printer(&printer)];
        assert_eq!(block.bottom_layer_count, Some(2));
        assert_eq!(block.wait_after_cure_sec.map(|s| s.value()), Some(2.0));
        assert_eq!(block.wait_after_lift_sec.map(|s| s.value()), Some(0.0));
        assert_eq!(block.wait_before_cure_sec.map(|s| s.value()), Some(0.5));
    }

    #[test]
    fn a_page_for_a_printer_we_do_not_ship_is_noted() {
        let reading = read(&[("resin-settings-some-other-printer", PAGE)]);
        assert!(reading.profiles.is_empty());
        assert!(matches!(
            reading.notes.as_slice(),
            [SourceNote::RowsForAMachineWeDoNotShip { rows: 5, .. }]
        ));
    }

    #[test]
    fn light_off_and_uv_power_columns_are_read() {
        let page = "|||||||||
|---|---|---|---|---|---|---|---|
||**Layer thickness**|**% UV Power**|**Exposure time (seconds)**|**Base layer count**|**Base layer exposure**|**Lift Height**|**Light off-delay / bottom (seconds)**|**Lift speed / retract speed**|
|[Premium White](x)|50 µm|40|2,0|2|25|6|1 / 3|90 / 120|
";
        let reading = read(&[("anycubic-photon-m3-plus-resin", page)]);
        let ConvertedProfile::Material(m) = &reading.profiles[0] else {
            panic!("a material");
        };
        let printer = PrinterId::new("anycubic-photon-mono-x-6k-m3-plus").unwrap();
        let block = m.per_printer[&CalibrationTarget::from_printer(&printer)];
        assert_eq!(block.wait_before_cure_sec.map(|s| s.value()), Some(1.0));
        assert_eq!(block.bottom_wait_before_cure_sec.map(|s| s.value()), Some(3.0));
        assert_eq!(block.light_pwm.map(|p| p.percent()), Some(40.0));
    }

    #[test]
    fn every_page_names_a_printer_we_ship() {
        let ours = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/profiles/printers");
        for (_, printer) in PAGES {
            assert!(
                ours.join(format!("{printer}.toml")).is_file(),
                "{printer} is not a printer we ship"
            );
        }
    }
}
