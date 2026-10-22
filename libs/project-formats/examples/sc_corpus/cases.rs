// SPDX-License-Identifier: Apache-2.0
//! Synthetic corpus definitions. Expected dimensions/volumes are analytic inputs,
//! never learned from whatever the current implementation happens to return.
use std::sync::Arc;

use sc_mesh_core::transform::{AffineTransform3D as A, Transform3D as T};
use sc_mesh_core::{IndexedMesh, IndexedTriangle as F, Vertex};
use sc_mesh_fixtures::{
    cuboid_with_y_tunnel, cuboid_with_z_tunnel, named, unit_cube_mesh,
    unit_cube_with_collinear_faces, unit_cube_with_flipped_faces,
};
use sc_scene::{LeafPayload, Scene};
use sc_scene_fixtures::{leaves_as_roots, named_trio_under_group_root};

pub(super) struct Case {
    pub id: &'static str,
    pub scene: Scene,
    pub bounds: [[f64; 3]; 2],
    pub volume: Option<f64>,
    pub edges_ok: bool,
    pub areas_ok: bool,
    // z sample, total polygon area, number of holes, number of polygons.
    pub probe: Option<(f64, f64, usize, usize)>,
    pub source: &'static str,
    pub intent: &'static str,
    pub formats: &'static [&'static str],
}

pub(super) fn transform(scale: [f64; 3], offset: [f64; 3]) -> T {
    T::Affine(A::from_translation(offset).compose(&A::from_non_uniform_scale(scale)))
}

pub(super) fn box_mesh(size: [f64; 3], center: [f64; 3]) -> IndexedMesh {
    sc_scene::bake_mesh_to_world(&unit_cube_mesh(), &transform(size, center)).unwrap()
}

fn combine(mut a: IndexedMesh, b: IndexedMesh, reverse_b: bool) -> IndexedMesh {
    let offset = u32::try_from(a.vertices.len()).unwrap();
    a.vertices.extend(b.vertices);
    a.faces.extend(b.faces.into_iter().map(|f| {
        let mut indices = f.vertices.map(|i| i.get() + offset);
        if reverse_b {
            indices.swap(1, 2);
        }
        F::from_raw_indices(indices)
    }));
    a
}

fn extruded_l() -> IndexedMesh {
    // CCW L polygon, area 20*5 + 5*15 = 175 mm², height 10 mm.
    let xy = [
        [0.0, 0.0],
        [20.0, 0.0],
        [20.0, 5.0],
        [5.0, 5.0],
        [5.0, 20.0],
        [0.0, 20.0],
    ];
    let vertices = [0.0, 10.0]
        .into_iter()
        .flat_map(|z| xy.map(|p| Vertex::new([p[0], p[1], z])))
        .collect();
    let top = [[0, 1, 2], [0, 2, 3], [0, 3, 5], [3, 4, 5]];
    let mut faces = Vec::new();
    for [a, b, c] in top {
        faces.push(F::from_raw_indices([a, c, b]));
        faces.push(F::from_raw_indices([a + 6, b + 6, c + 6]));
    }
    for a in 0..6 {
        let b = (a + 1) % 6;
        faces.push(F::from_raw_indices([a, b, b + 6]));
        faces.push(F::from_raw_indices([a, b + 6, a + 6]));
    }
    IndexedMesh {
        meta: None,
        vertices,
        faces,
    }
}

fn one(
    id: &'static str,
    mesh: IndexedMesh,
    bounds: [[f64; 3]; 2],
    volume: f64,
    probe: (f64, f64, usize, usize),
    source: &'static str,
    intent: &'static str,
) -> Case {
    Case {
        id,
        scene: Scene::from(named(mesh, id)),
        bounds,
        volume: Some(volume),
        edges_ok: true,
        areas_ok: true,
        probe: Some(probe),
        source,
        intent,
        formats: &["3mf"],
    }
}

pub(super) fn cases() -> Vec<Case> {
    let cube = || box_mesh([10.0; 3], [5.0; 3]);
    let mut out = vec![
        one(
            "solid-cube-10mm",
            cube(),
            [[0.0; 3], [10.0; 3]],
            1000.0,
            (5.0, 100.0, 0, 1),
            "sc_mesh_fixtures::unit_cube_mesh + scale/translation",
            "Dimensiones, origen, formatos equivalentes",
        ),
        one(
            "thin-wall-0_2mm",
            box_mesh([20.0, 0.2, 10.0], [10.0, 0.1, 5.0]),
            [[0.0; 3], [20.0, 0.2, 10.0]],
            40.0,
            (5.0, 4.0, 0, 1),
            "sc_mesh_fixtures::unit_cube_mesh + scale/translation",
            "Pared fina; espesor 0.2 mm; no implica imprimibilidad física",
        ),
        one(
            "concave-l",
            extruded_l(),
            [[0.0; 3], [20.0, 20.0, 10.0]],
            1750.0,
            (5.0, 175.0, 0, 1),
            "examples/sc_corpus/cases.rs::extruded_l",
            "Concavidad real, no dos cajas solapadas",
        ),
        one(
            "sealed-cavity-2mm",
            combine(
                box_mesh([20.0; 3], [10.0; 3]),
                box_mesh([16.0; 3], [10.0; 3]),
                true,
            ),
            [[0.0; 3], [20.0; 3]],
            3904.0,
            (10.0, 144.0, 1, 1),
            "unit_cube_mesh: outer20 minus reversed inner16",
            "Dos shells anidados; pared 2 mm; cavidad sin drenaje, NO lista para imprimir",
        ),
        one(
            "vertical-tunnel-8mm",
            sc_scene::bake_mesh_to_world(
                &cuboid_with_z_tunnel(),
                &transform([10.0; 3], [10.0, 10.0, 0.0]),
            )
            .unwrap(),
            [[0.0; 3], [20.0; 3]],
            6720.0,
            (10.0, 336.0, 1, 1),
            "sc_mesh_fixtures::cuboid_with_z_tunnel + scale10",
            "Túnel pasante vertical; agujero cuadrado 8 mm; no cavidad sellada",
        ),
        one(
            "horizontal-tunnel-8mm",
            sc_scene::bake_mesh_to_world(
                &cuboid_with_y_tunnel(),
                &transform([10.0; 3], [10.0, 10.0, 0.0]),
            )
            .unwrap(),
            [[0.0; 3], [20.0; 3]],
            6720.0,
            (10.0, 240.0, 0, 2),
            "sc_mesh_fixtures::cuboid_with_y_tunnel + scale10",
            "Techo/puente sobre hueco; corte central dividido en dos polígonos",
        ),
        one(
            "disconnected-solids",
            combine(cube(), box_mesh([10.0; 3], [25.0, 5.0, 5.0]), false),
            [[0.0; 3], [30.0, 10.0, 10.0]],
            2000.0,
            (5.0, 200.0, 0, 2),
            "unit_cube_mesh: two disconnected boxes",
            "Dos componentes en una malla, no dos objetos",
        ),
    ];
    out[0].formats = &["stl", "stla", "obj", "3mf"];
    out[3].formats = &["stl", "3mf"];
    let mut unitless = one(
        "unitless-numbers-one",
        box_mesh([1.0; 3], [0.5; 3]),
        [[0.0; 3], [1.0; 3]],
        1.0,
        (0.5, 1.0, 0, 1),
        "unit_cube_mesh + translation0.5",
        "STL almacena números 0..1 sin unidad; SC los interpreta como 1 mm, no una pulgada",
    );
    unitless.formats = &["stl"];
    out.push(unitless);
    for (id, scale, bounds, volume, area, intent) in [
        (
            "nonuniform-scale",
            [2.0, 0.5, 1.5],
            [[0.0; 3], [20.0, 5.0, 15.0]],
            1500.0,
            100.0,
            "Transformación conservada, no escala horneada",
        ),
        (
            "mirrored-scale",
            [-1.0, 1.0, 1.0],
            [[-10.0, 0.0, 0.0], [0.0, 10.0, 10.0]],
            1000.0,
            100.0,
            "Determinante negativo; bake corrige winding",
        ),
    ] {
        let mut c = one(
            id,
            cube(),
            bounds,
            volume,
            (5.0, area, 0, 1),
            "unit_cube_mesh + scene local transform",
            intent,
        );
        c.scene = leaves_as_roots([(named(cube(), id), transform(scale, [0.0; 3]))]);
        out.push(c);
    }
    let mut rotated = one(
        "rotated-z-90",
        box_mesh([10.0, 20.0, 5.0], [0.0, 0.0, 2.5]),
        [[-10.0, -5.0, 0.0], [10.0, 5.0, 5.0]],
        1000.0,
        (2.0, 200.0, 0, 1),
        "unit_cube_mesh + scene rotation 90deg",
        "Rotación de escena, AABB y volumen físico",
    );
    rotated.scene = leaves_as_roots([(
        box_mesh([10.0, 20.0, 5.0], [0.0, 0.0, 2.5]),
        T::Affine(A::from_rotation_z(std::f64::consts::FRAC_PI_2)),
    )]);
    out.push(rotated);
    out.push(Case {
        id: "grouped-trio",
        scene: named_trio_under_group_root([10.0, 10.0, 0.5]),
        bounds: [[4.0, 9.0, 0.0], [16.0, 11.0, 2.5]],
        volume: Some(14.44),
        edges_ok: true,
        areas_ok: true,
        probe: None,
        source: "sc_scene_fixtures::named_trio_under_group_root",
        intent: "Grupo y nombres Cube/ZTunnel/YTunnel; geometría y poses distintas",
        formats: &["3mf", "obj"],
    });
    let mut shared = Scene::new();
    let rid = shared.add_mesh_resource(named(cube(), "shared-cube"));
    for x in [0.0, 20.0] {
        let leaf = shared
            .add_leaf(
                None,
                transform([1.0; 3], [x, 0.0, 0.0]),
                LeafPayload::MeshResource(rid),
            )
            .unwrap();
        shared.add_root(leaf).unwrap();
    }
    out.push(Case {id:"shared-mesh-two-leaves",scene:shared,bounds:[[0.0;3],[30.0,10.0,10.0]],volume:Some(2000.0),
        edges_ok:true,areas_ok:true,probe:Some((5.0,200.0,0,2)),source:"Scene::add_mesh_resource + two leaves",
        intent:"Un mesh resource compartido, dos objetos con transforms diferentes; no Reference node",
        formats:&["3mf"]});
    out.push(Case {id:"stacked-island",scene:leaves_as_roots([(cube(),T::Identity),(cube(),transform([1.0;3],[0.0,0.0,15.0]))]),
        bounds:[[0.0;3],[10.0,10.0,25.0]],volume:Some(2000.0),edges_ok:true,areas_ok:true,
        probe:Some((12.0,0.0,0,0)),source:"unit_cube_mesh + two translated leaves",
        intent:"Vacío entre Z10 y15; segundo sólido comienza sin contacto en Z15, caso futuro de islas",formats:&["3mf"]});
    for (id, mesh, edges_ok, areas_ok, intent) in [
        (
            "open-missing-face",
            {
                let mut m = cube();
                m.faces.pop();
                m
            },
            false,
            true,
            "Carga aceptada pero borde abierto; slicer debe rechazar",
        ),
        (
            "flipped-faces",
            unit_cube_with_flipped_faces(),
            false,
            true,
            "Orientación inconsistente; no autorreparar al importar sin informar",
        ),
        (
            "collinear-triangles",
            unit_cube_with_collinear_faces(),
            true,
            false,
            "Índices válidos pero caras de área cero; degenerado, no archivo corrupto",
        ),
        (
            "nonmanifold-duplicate-face",
            {
                let mut m = cube();
                m.faces.push(m.faces[0]);
                m
            },
            false,
            true,
            "Aristas con tres caras; no confundir cierre visual con manifold",
        ),
    ] {
        let bound = if id == "flipped-faces" || id == "collinear-triangles" {
            [[-0.5; 3], [0.5; 3]]
        } else {
            [[0.0; 3], [10.0; 3]]
        };
        out.push(Case {
            id,
            scene: Scene::from(mesh),
            bounds: bound,
            volume: None,
            edges_ok,
            areas_ok,
            probe: None,
            source: "sc_mesh_fixtures variants / deterministic face removal or duplication",
            intent,
            formats: &["stl"],
        });
    }
    out
}

pub(super) fn reference_scene() -> Scene {
    let mut scene = Scene::new();
    let leaf = scene
        .add_leaf(
            None,
            T::Identity,
            LeafPayload::Mesh(Arc::new(box_mesh([10.0; 3], [5.0; 3]))),
        )
        .unwrap();
    let resource = scene.register_subtree_resource(leaf).unwrap();
    for x in [0.0, 20.0] {
        let reference = scene
            .add_reference(None, transform([1.0; 3], [x, 0.0, 0.0]), resource)
            .unwrap();
        scene.add_root(reference).unwrap();
    }
    scene
}
