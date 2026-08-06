/*
 * SPDX-FileCopyrightText: 2026 MyMiniFactory Ltd, 2026 CTRLback SCCL
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! Generate the Windows `.ico` from the shared SVG logo.
//!
//! Run on demand after the logo changes: `cargo run -p sc-icon-gen`.

use std::path::PathBuf;

use resvg::tiny_skia;
use resvg::usvg;

const SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];
const SVG_RELATIVE: &str = "packages/shared/it.soulcrafted.slicer.svg";
const ICO_RELATIVE: &str = "apps/slicer-gui/assets/icons/it.soulcrafted.slicer.ico";

fn main() {
    if let Err(err) = run() {
        eprintln!("icon-gen: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let workspace_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .ok_or("cannot locate the workspace root from CARGO_MANIFEST_DIR")?
        .to_path_buf();

    let svg_path = workspace_root.join(SVG_RELATIVE);
    let ico_path = workspace_root.join(ICO_RELATIVE);

    let svg_data =
        std::fs::read(&svg_path).map_err(|e| format!("reading {}: {e}", svg_path.display()))?;
    let tree = usvg::Tree::from_data(&svg_data, &usvg::Options::default())
        .map_err(|e| format!("parsing {}: {e}", svg_path.display()))?;

    let svg_max = tree.size().width().max(tree.size().height());
    if svg_max <= 0.0 {
        return Err("the SVG reports a non-positive size".into());
    }

    let mut icon_dir = ico::IconDir::new(ico::ResourceType::Icon);
    for size in SIZES {
        let mut pixmap =
            tiny_skia::Pixmap::new(size, size).ok_or("failed to allocate the render pixmap")?;
        let scale = size as f32 / svg_max;
        resvg::render(
            &tree,
            tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );

        // .ico wants straight alpha; tiny-skia stores it premultiplied.
        let mut rgba = Vec::with_capacity((size as usize) * (size as usize) * 4);
        for px in pixmap.pixels() {
            let c = px.demultiply();
            rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
        }

        let image = ico::IconImage::from_rgba_data(size, size, rgba);
        icon_dir.add_entry(ico::IconDirEntry::encode(&image)?);
    }

    if let Some(parent) = ico_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("creating {}: {e}", parent.display()))?;
    }
    let file = std::fs::File::create(&ico_path)
        .map_err(|e| format!("creating {}: {e}", ico_path.display()))?;
    icon_dir
        .write(file)
        .map_err(|e| format!("writing {}: {e}", ico_path.display()))?;

    println!("wrote {} ({} sizes)", ico_path.display(), SIZES.len());
    Ok(())
}
