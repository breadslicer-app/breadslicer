// SPDX-License-Identifier: Apache-2.0
//! SC-24: deterministic small corpus, checked through real core APIs.
//! Usage: cargo run --locked -p sc-project-formats --example sc_corpus -- <empty-output-dir>
#[path = "sc_corpus/cases.rs"]
mod cases;

use std::error::Error;
use std::fs;
use std::io::Cursor;
use std::path::Path;

use sc_mesh_core::IndexedMesh;
use sc_mesh_formats::{filetypes::WriteFormat, load_mesh_from_path, scene_writer::write_scene};
use sc_project_formats::{
    EmbedProfiles, EmbeddedProfile, ProjectFileType, ProjectPayload, read_project, write_project,
};
use sc_scene::{CsgOp, LeafPayload, Scene};
use sc_sl1cer::profile::{
    AaLevel, BuildVolume, LayerSchedule, Mirror, PackingMode, PixelResolution, SliceProfile,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn close(got: f64, want: f64) {
    assert!(
        (got - want).abs() <= 1e-4_f64.max(want.abs() * 1e-5),
        "got {got}, expected {want}"
    );
}

fn bounds(mesh: &IndexedMesh) -> [[f64; 3]; 2] {
    let mut b = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    for v in mesh.vertices() {
        for i in 0..3 {
            b[0][i] = b[0][i].min(f64::from(v[i]));
            b[1][i] = b[1][i].max(f64::from(v[i]));
        }
    }
    b
}

fn check_scene(scene: &Scene, c: &cases::Case) -> Value {
    let mesh = scene.merge_meshes().unwrap().unwrap();
    let actual_bounds = bounds(&mesh);
    for (got, want) in actual_bounds
        .iter()
        .flatten()
        .zip(c.bounds.iter().flatten())
    {
        close(*got, *want);
    }
    let faces = mesh.validated_faces().unwrap();
    assert_eq!(
        faces.validate_edge_manifold().is_ok(),
        c.edges_ok,
        "{} edges",
        c.id
    );
    assert_eq!(
        faces.validate_face_areas().is_ok(),
        c.areas_ok,
        "{} areas",
        c.id
    );
    if let Some(volume) = c.volume {
        close(scene.world_volume_mm3().unwrap(), volume);
    }
    json!({"bounds_mm":actual_bounds,"triangles":mesh.faces().len(),"edge_manifold":c.edges_ok,"positive_face_areas":c.areas_ok})
}

fn profile_at(z: f64) -> SliceProfile {
    // One slab from Z0 to2z; its center is exactly the analytic probe plane z.
    SliceProfile::new(
        BuildVolume::new(100.0, 100.0, 100.0).unwrap(),
        PixelResolution::new(100, 100).unwrap(),
        LayerSchedule::uniform(2.0 * z, 2.0 * z, 1).unwrap(),
        Mirror::default(),
        AaLevel::Off,
        PackingMode::None,
    )
}

fn check_probe(scene: &Scene, probe: (f64, f64, usize, usize)) -> Value {
    let (z, area, holes, polygons) = probe;
    let merged = Scene::from(scene.merge_meshes().unwrap().unwrap());
    let stacks = sc_sl1cer::slice_scene(&merged, &profile_at(z)).unwrap();
    let layer = &stacks[0].stack().layers()[0];
    close(layer.z_mm(), z);
    let actual_area: f64 = layer
        .polygons()
        .iter()
        .map(|p| p.outer().signed_area() + p.holes().iter().map(|h| h.signed_area()).sum::<f64>())
        .sum();
    close(actual_area, area);
    assert_eq!(layer.polygons().len(), polygons);
    assert_eq!(
        layer
            .polygons()
            .iter()
            .map(|p| p.holes().len())
            .sum::<usize>(),
        holes
    );
    json!({"z_mm":z,"area_mm2":area,"holes":holes,"polygons":polygons,"status":"passed"})
}

fn identity(path: &Path) -> Value {
    let data = fs::read(path).unwrap();
    json!({"path":path.file_name().unwrap().to_str().unwrap(),"bytes":data.len(),"sha256":format!("{:x}",Sha256::digest(data))})
}

fn main() -> Result<(), Box<dyn Error>> {
    let arg = std::env::args()
        .nth(1)
        .ok_or("Pass a new or empty output directory")?;
    let output = Path::new(&arg);
    fs::create_dir_all(output)?;
    if fs::read_dir(output)?.next().is_some() {
        return Err(
            "Output directory must be empty; existing artifacts are never overwritten".into(),
        );
    }
    let mut entries = Vec::new();
    for case in cases::cases() {
        println!("Checking {}", case.id);
        let original = check_scene(&case.scene, &case);
        let probe = case.probe.map(|p| check_probe(&case.scene, p));
        let mut artifacts = Vec::new();
        for extension in case.formats {
            let format = match *extension {
                "stl" => WriteFormat::StlBin,
                "stla" => WriteFormat::StlAscii,
                "obj" => WriteFormat::Obj,
                _ => WriteFormat::ThreeMf,
            };
            let path = output.join(format!("{}.{extension}", case.id));
            write_scene(&case.scene, &path, format)?;
            let loaded = load_mesh_from_path(&path, None, Some(1_000_000))?;
            check_scene(&loaded.graph, &case);
            if let Some(p) = case.probe {
                check_probe(&loaded.graph, p);
            }
            if *extension == "3mf" {
                assert_eq!(
                    loaded.graph.reachable_leaf_count(),
                    case.scene.reachable_leaf_count()
                );
                if case.id == "shared-mesh-two-leaves" {
                    assert_eq!(loaded.graph.mesh_resources_iter().count(), 1);
                }
            }
            artifacts.push(identity(&path));
        }
        if !case.edges_ok {
            assert!(
                matches!(
                    sc_sl1cer::slice_scene(&case.scene, &profile_at(0.25)),
                    Err(sc_sl1cer::error::SliceSceneError::LeavesValidationFailed(_))
                ),
                "invalid mesh must fail geometry validation, not for an unrelated reason"
            );
        }
        entries.push(json!({"id":case.id,"source":case.source,"license":"Apache-2.0","intent":case.intent,
            "units":"mm; STL/OBJ have no stored unit","expected":{"bounds_mm":case.bounds,"volume_mm3":case.volume,
            "edge_manifold":case.edges_ok,"positive_face_areas":case.areas_ok},"verified":original,"slice_probe":probe,"artifacts":artifacts}));
    }
    // Same numeric coordinates as unitless-numbers-one.stl, explicitly inches.
    // Use the existing typed 3MF vocabulary, not a hand-rolled ZIP/XML exporter.
    let one = Scene::from(cases::box_mesh([1.0; 3], [0.5; 3]));
    let mut model = sc_mesh_formats::threemf::scene_to_model_document(&one)?.document;
    model.unit = sc_3mf::model::Unit::Inch;
    let mut xml = Vec::new();
    sc_3mf::model::write_model(&model, &mut xml)?;
    let mut package = sc_3mf::package::Package::new();
    package.add_part(
        sc_3mf::package::PartName::new("/3D/3dmodel.model")?,
        sc_3mf::names::MODEL_PART_CONTENT_TYPE,
        xml,
    )?;
    package.add_root_relationship(sc_3mf::package::Relationship {
        id: "rel-1".into(),
        rel_type: sc_3mf::names::START_PART_REL_TYPE.into(),
        target: "/3D/3dmodel.model".into(),
    });
    let mut inch_bytes = Cursor::new(Vec::new());
    package.write_to(&mut inch_bytes)?;
    let path = output.join("explicit-inch-cube.3mf");
    fs::write(&path, inch_bytes.into_inner())?;
    let inch_scene = load_mesh_from_path(&path, None, Some(1_000_000))?.graph;
    for (got, want) in bounds(&inch_scene.merge_meshes()?.unwrap())
        .iter()
        .flatten()
        .zip([[0.0; 3], [25.4; 3]].iter().flatten())
    {
        close(*got, *want);
    }
    close(inch_scene.world_volume_mm3()?, 25.4_f64.powi(3));
    entries.push(json!({"id":"explicit-inch-cube","source":"unit_cube_mesh + sc_3mf ModelDocument unit=inch","license":"Apache-2.0",
        "intent":"Mismos números 0..1 que STL unitless; 3MF declara inch y debe medir 25.4 mm",
        "expected":{"bounds_mm":[[0,0,0],[25.4,25.4,25.4]],"volume_mm3":25.4_f64.powi(3),"stored_unit":"inch"},
        "verified":"core 3MF loader scales dimensions and volume","artifacts":[identity(&path)]}));
    // A real project with synthetic profiles: these are test data, never machine settings.
    let scene = sc_scene_fixtures::named_trio_under_group_root([10.0, 10.0, 0.5]);
    let payload = ProjectPayload {
        printer: Some("test-printer".parse()?),
        resin: Some("test-resin".parse()?),
        first_layer_height: Some(0.05.try_into()?),
        embedded: vec![
            EmbeddedProfile::Printer {
                id: "test-printer".parse()?,
                bytes: include_bytes!("../tests/fixtures/test-printer.toml").to_vec(),
            },
            EmbeddedProfile::Material {
                id: "test-resin".parse()?,
                bytes: include_bytes!("../tests/fixtures/test-resin.toml").to_vec(),
            },
        ],
        ..Default::default()
    };
    let mut bytes = Vec::new();
    write_project(
        &scene,
        &payload,
        EmbedProfiles::Yes,
        None,
        None,
        ProjectFileType::ThreeMf,
        &mut bytes,
    )?;
    let loaded = read_project(&mut Cursor::new(&bytes), ProjectFileType::ThreeMf)?;
    assert_eq!(loaded.payload.as_ref(), Some(&payload));
    assert_eq!(loaded.scene.reachable_leaf_count(), 3);
    close(loaded.scene.world_volume_mm3()?, 14.44);
    let path = output.join("project-embedded-profiles.3mf");
    fs::write(&path, &bytes)?;
    entries.push(json!({"id":"project-embedded-profiles","source":"sc_scene_fixtures + project-formats/tests/fixtures profiles",
        "license":"Apache-2.0","intent":"Roundtrip de proyecto con dos perfiles sintéticos; no ajustes de impresora real",
        "expected":{"leaves":3,"volume_mm3":14.44,"printer":"test-printer","resin":"test-resin","embedded_profiles":2,"first_layer_height_mm":0.05},
        "verified":"core project read/write and payload exact match","artifacts":[identity(&path)]}));

    // A Reference node differs from two leaves sharing a MeshResource. Keep this
    // recipe in Rust: importing 3MF may normalize its representation into groups.
    let referenced = cases::reference_scene();
    assert!(matches!(
        sc_sl1cer::slice_scene(&referenced, &profile_at(5.0)),
        Err(sc_sl1cer::error::SliceSceneError::SubtreeReferenceNotYetSupported { .. })
    ));
    close(referenced.world_volume_mm3()?, 2000.0);
    entries.push(json!({"id":"subtree-reference-core-only","source":"cases::reference_scene","license":"Apache-2.0",
        "intent":"Receta core: dos instancias de subtree; rechazo explícito al slice, no simular soporte por un export que normaliza",
        "expected":{"slice":"reject unsupported reference","volume_mm3":2000},"verified":"passed","artifacts":[]}));
    let mut csg = Scene::new();
    let leaf = csg.add_leaf(
        None,
        sc_mesh_core::transform::Transform3D::Identity,
        LeafPayload::Mesh(std::sync::Arc::new(sc_mesh_fixtures::unit_cube_mesh())),
    )?;
    let root = csg.add_csg(
        None,
        sc_mesh_core::transform::Transform3D::Identity,
        CsgOp::Union,
        vec![leaf],
    )?;
    csg.add_root(root)?;
    assert!(matches!(
        sc_sl1cer::slice_scene(&csg, &profile_at(0.25)),
        Err(sc_sl1cer::error::SliceSceneError::CsgNotYetSupported { .. })
    ));
    assert!(csg.merge_meshes().is_err());
    entries.push(json!({"id":"unevaluated-csg-core-only","source":"Scene::add_csg(Union)","license":"Apache-2.0",
        "intent":"Enum CSG no implica motor booleano; rechazo al slice/bake", "expected":{"slice":"reject unsupported CSG","bake":"reject unevaluated CSG"},"verified":"passed","artifacts":[]}));

    // Malformed bytes, not merely a geometrically open but parseable mesh.
    let mut truncated = vec![0_u8; 84];
    truncated[80..84].copy_from_slice(&1_u32.to_le_bytes());
    for (id, name, bytes) in [
        ("truncated-stl", "invalid-truncated.stl", truncated),
        (
            "invalid-3mf",
            "invalid-container.3mf",
            b"not a zip".to_vec(),
        ),
        (
            "invalid-obj",
            "invalid-index.obj",
            b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 99\n".to_vec(),
        ),
    ] {
        let path = output.join(name);
        fs::write(&path, bytes)?;
        assert!(
            load_mesh_from_path(&path, None, Some(1_000_000)).is_err(),
            "{name} should fail parsing"
        );
        entries.push(json!({"id":id,"source":"synthetic invalid bytes in sc_corpus.rs","license":"Apache-2.0",
            "intent":"Error de archivo controlado, no crashear ni reparar silenciosamente","expected":{"load":"reject"},"verified":"rejected as expected","artifacts":[identity(&path)]}));
    }
    let manifest = json!({"schema":1,"task":"SC-24","generator":"cargo run --locked -p sc-project-formats --example sc_corpus -- <empty-output-dir>",
        "provenance":"Existing Apache-2.0 sc-mesh-fixtures/sc-scene-fixtures plus original synthetic definitions; preserve upstream authorship in repository history and licenses.",
        "limits":["Core checks only: not native GUI tests or physical print validation","Slice probes check analytic cross-sections, not full pixel/raster equivalence","No private meshes, research bundles or external downloads","Large performance meshes intentionally excluded"],"cases":entries});
    fs::write(
        output.join("MANIFEST.json"),
        serde_json::to_string_pretty(&manifest)? + "\n",
    )?;
    println!(
        "Verified {} cases",
        manifest["cases"].as_array().unwrap().len()
    );
    Ok(())
}
