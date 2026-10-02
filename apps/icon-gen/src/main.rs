/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Generate the packaging icons from the shared SVG logo: the Windows `.ico`
//! that the GUI executable embeds, and the macOS `.icns` that the application
//! bundle carries.
//!
//! Run on demand after the logo changes: `cargo run -p sc-icon-gen`.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use resvg::tiny_skia;
use resvg::usvg;

const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// Each render size and the `.icns` entries encoded from it. macOS keys an
/// entry on a size-and-density pair, so 1 render serves 2 entries wherever the
/// pair agrees on pixels: a 32 pixel square is both 32x32 at 1x and 16x16 at
/// 2x, and a display picks whichever matches its own density.
///
/// The set matches what `iconutil` writes for the same images, down to the
/// OSType of every entry. It matters at the 2 smallest sizes, where the
/// alternatives `icp4` and `icp5` are the superseded small-icon types that
/// `iconutil` no longer writes, and those sizes are the ones the Finder list
/// view, the sidebar and the menu bar ask for.
const ICNS_ENTRIES: [(u32, &[icns::IconType]); 7] = [
    (16, &[icns::IconType::RGBA32_16x16_ARGB]),
    (
        32,
        &[
            icns::IconType::RGBA32_16x16_2x,
            icns::IconType::RGBA32_16x16_2x_ARGB,
        ],
    ),
    (64, &[icns::IconType::RGBA32_32x32_2x]),
    (128, &[icns::IconType::RGBA32_128x128]),
    (
        256,
        &[
            icns::IconType::RGBA32_128x128_2x,
            icns::IconType::RGBA32_256x256,
        ],
    ),
    (
        512,
        &[
            icns::IconType::RGBA32_256x256_2x,
            icns::IconType::RGBA32_512x512,
        ],
    ),
    (1024, &[icns::IconType::RGBA32_512x512_2x]),
];

const SVG_RELATIVE: &str = "packages/shared/it.soulcrafted.slicer.svg";
const ICO_RELATIVE: &str = "apps/slicer-gui/assets/icons/it.soulcrafted.slicer.ico";
const ICNS_RELATIVE: &str = "packages/macos/it.soulcrafted.slicer.icns";

fn main() {
    if let Err(err) = run() {
        eprintln!("icon-gen: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .ok_or("cannot locate the workspace root from CARGO_MANIFEST_DIR")?
        .to_path_buf();

    let svg_path = workspace_root.join(SVG_RELATIVE);
    let svg_data =
        fs::read(&svg_path).map_err(|e| format!("reading {}: {e}", svg_path.display()))?;
    let tree = usvg::Tree::from_data(&svg_data, &usvg::Options::default())
        .map_err(|e| format!("parsing {}: {e}", svg_path.display()))?;

    let svg_max = tree.size().width().max(tree.size().height());
    if svg_max <= 0.0 {
        return Err("the SVG reports a non-positive size".into());
    }
    let logo = Logo {
        tree: &tree,
        svg_max,
    };

    write_ico(&logo, &workspace_root.join(ICO_RELATIVE))?;
    write_icns(&logo, &workspace_root.join(ICNS_RELATIVE))?;
    Ok(())
}

/// The parsed logo, with the longer of its 2 sides. Rendering scales that
/// side to the requested square, so a logo that is not square keeps its aspect
/// ratio and is padded rather than stretched.
struct Logo<'a> {
    tree: &'a usvg::Tree,
    svg_max: f32,
}

impl Logo<'_> {
    /// Renders the logo into a straight-alpha RGBA buffer, `size` pixels
    /// square.
    fn render_rgba(&self, size: u32) -> Result<Vec<u8>, Box<dyn Error>> {
        let mut pixmap =
            tiny_skia::Pixmap::new(size, size).ok_or("failed to allocate the render pixmap")?;
        let scale = size as f32 / self.svg_max;
        resvg::render(
            self.tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );

        // Both container formats want straight alpha; tiny-skia stores it
        // premultiplied.
        let mut rgba = Vec::with_capacity((size as usize) * (size as usize) * 4);
        for px in pixmap.pixels() {
            let c = px.demultiply();
            rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }
        Ok(rgba)
    }
}

fn write_ico(logo: &Logo<'_>, path: &Path) -> Result<(), Box<dyn Error>> {
    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in ICO_SIZES {
        let image = ico::IconImage::from_rgba_data(size, size, logo.render_rgba(size)?);
        icon_dir.add_entry(ico::IconDirEntry::encode(&image)?);
    }

    icon_dir
        .write(create_file(path)?)
        .map_err(|e| format!("writing {}: {e}", path.display()))?;

    println!("wrote {} ({} sizes)", path.display(), ICO_SIZES.len());
    Ok(())
}

fn write_icns(logo: &Logo<'_>, path: &Path) -> Result<(), Box<dyn Error>> {
    let mut family = icns::IconFamily::new();
    let mut entries = 0;
    for (size, icon_types) in ICNS_ENTRIES {
        let image =
            icns::Image::from_data(icns::PixelFormat::RGBA, size, size, logo.render_rgba(size)?)
                .map_err(|e| format!("building the {size}px image: {e}"))?;
        for icon_type in icon_types {
            family
                .add_icon_with_type(&image, *icon_type)
                .map_err(|e| format!("encoding the {size}px {icon_type:?} entry: {e}"))?;
            entries += 1;
        }
    }

    family
        .write(create_file(path)?)
        .map_err(|e| format!("writing {}: {e}", path.display()))?;

    println!("wrote {} ({entries} entries)", path.display());
    Ok(())
}

/// Creates the file at `path`, and any directory above it that is missing.
fn create_file(path: &Path) -> Result<fs::File, Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    Ok(fs::File::create(path).map_err(|e| format!("creating {}: {e}", path.display()))?)
}

#[cfg(test)]
mod tests {
    use super::ICNS_ENTRIES;

    #[test]
    fn every_icns_entry_is_rendered_at_the_size_its_icon_type_expects() {
        // `add_icon_with_type` rejects a mismatch, but only when the generator
        // runs. This pins the table itself, where a wrong pairing is a silent
        // edit rather than a visible one.
        for (size, icon_types) in ICNS_ENTRIES {
            for icon_type in icon_types {
                assert_eq!(
                    (icon_type.pixel_width(), icon_type.pixel_height()),
                    (size, size),
                    "{icon_type:?} is listed under {size}px",
                );
            }
        }
    }

    #[test]
    fn the_icns_entries_carry_the_ostypes_iconutil_writes() {
        // Pinned against `iconutil`'s own output for these images. The 2
        // smallest entries are the reason: picking the plain 16x16 and 32x32
        // variants yields `icp4` and `icp5`, which iconutil no longer writes,
        // and nothing in the type names says so.
        let mut written: Vec<String> = ICNS_ENTRIES
            .iter()
            .flat_map(|(_, icon_types)| icon_types.iter().map(|t| t.ostype().to_string()))
            .collect();
        written.sort();
        assert_eq!(
            written,
            [
                "ic04", "ic05", "ic07", "ic08", "ic09", "ic10", "ic11", "ic12", "ic13", "ic14",
            ],
        );
    }

    #[test]
    fn no_icns_icon_type_is_listed_twice() {
        // A repeated type overwrites the entry already encoded under it, so the
        // family would come out 1 entry short of what the table reads like.
        let mut seen = Vec::new();
        for (_, icon_types) in ICNS_ENTRIES {
            for icon_type in icon_types {
                assert!(
                    !seen.contains(icon_type),
                    "{icon_type:?} is listed more than once",
                );
                seen.push(*icon_type);
            }
        }
    }
}
