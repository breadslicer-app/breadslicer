/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! `bodies`: the bodies "Split into bodies" makes of each model
//! (`sc_mesh_core::shells`), one line per file: triangles, bodies, the shells
//! in fragments, the cavities kept with their body, and the time to read the
//! file, to find the bodies and to cut their meshes.

use std::ffi::OsString;
use std::time::Instant;

use anyhow::Result;
use bpaf::{Parser, construct, long, positional};
use sc_mesh_core::shells::{Bodies, BodyKind};
use sc_mesh_formats::scene_writer::flatten_graph_to_mesh;
use sc_mesh_formats::{DEFAULT_MAX_MESH_FILE_SIZE_BYTES, load_mesh_from_path};
use serde_json::json;

use crate::commands::mesh_loader::display_warnings;
use crate::config::cmd_options::CmdOptions;
use crate::filesystem::validate_sourcefile_path;

#[derive(Clone, Debug)]
pub(crate) struct BodiesArgs {
    json: bool,
    sources: Vec<OsString>,
}

pub(crate) fn get_bodies_options() -> impl Parser<CmdOptions> {
    let json = long("json")
        .help("Print one JSON object per file instead of text.")
        .switch();
    let sources = positional::<OsString>("MODEL")
        .help("The 3D model files to split")
        .some("at least 1 model file");
    construct!(BodiesArgs { json, sources }).map(CmdOptions::Bodies)
}

pub(crate) fn bodies(args: BodiesArgs) -> Result<()> {
    if !args.json {
        println!("triangles\tbodies\tfragment shells\tcavities\tread ms\tfind ms\tcut ms\tfile");
    }
    for source in &args.sources {
        let path = validate_sourcefile_path(source.into())?;
        let t = Instant::now();
        let loaded = load_mesh_from_path(&path, None, Some(DEFAULT_MAX_MESH_FILE_SIZE_BYTES))?;
        display_warnings(&loaded.warnings, &mut std::io::stderr());
        let flat = flatten_graph_to_mesh(&loaded.graph)?;
        let read_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let found = Bodies::of_mesh(&flat.mesh);
        let find_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let meshes = found.meshes(&flat.mesh);
        let cut_ms = t.elapsed().as_secs_f64() * 1e3;
        drop(meshes);
        let fragment_shells: usize = found
            .bodies()
            .iter()
            .filter(|b| b.kind == BodyKind::Fragments)
            .map(|b| b.shells)
            .sum();
        let cavities: usize = found.bodies().iter().map(|b| b.cavities).sum();
        let triangles = flat.mesh.faces().len();
        let file = path.display();
        if args.json {
            println!(
                "{}",
                json!({
                    "file": file.to_string(),
                    "triangles": triangles,
                    "shells": found.shells().count(),
                    "bodies": found.count(),
                    "fragment_shells": fragment_shells,
                    "cavities": cavities,
                    "read_ms": read_ms,
                    "find_ms": find_ms,
                    "cut_ms": cut_ms,
                })
            );
        } else {
            println!(
                "{triangles}\t{}\t{fragment_shells}\t{cavities}\t{read_ms:.0}\t{find_ms:.1}\t{cut_ms:.1}\t{file}",
                found.count()
            );
        }
    }
    Ok(())
}
