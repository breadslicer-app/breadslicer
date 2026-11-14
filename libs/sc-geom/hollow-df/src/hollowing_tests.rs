//! DragonFruit's hollowing tests (`hollowing.rs` `mod tests`), ported.

use super::*;
use crate::shapes::{cube, icosphere, merged};

#[test]
fn slab_surface_matches_exhaustive_distance_union() {
    // Non-multiple-of-eight depth exercises the final partial slab; the
    // cell-major reference has no triangle AABB clipping.
    let grid = GridSpec {
        nx: 19,
        ny: 17,
        nz: 23,
        voxel_mm: 0.8,
        min: Vec3::new(-7.0, -7.0, -7.0),
    };
    let mut mesh = icosphere(5.0, 1);
    mesh.triangles.extend(mesh.triangles.clone()); // overlapping visits
    for rotation in [[0.0, 0.0, 0.0, 1.0], [0.0, 0.382_683_43, 0.0, 0.923_879_5]] {
        let triangles: Vec<_> = mesh
            .triangles
            .iter()
            .map(|t| {
                TriangleCache::from_points(
                    mesh.positions[t[0] as usize].rotate_by_quat(rotation),
                    mesh.positions[t[1] as usize].rotate_by_quat(rotation),
                    mesh.positions[t[2] as usize].rotate_by_quat(rotation),
                )
            })
            .collect();
        let threshold = (3.0f32).sqrt() * grid.voxel_mm * 0.5;
        let expected: Vec<_> = (0..grid.cell_count())
            .map(|i| {
                let (x, y, z) = grid.coords(i);
                let p = grid.center_world(x, y, z);
                triangles
                    .iter()
                    .any(|t| point_triangle_distance(p, t.a, t.b, t.c) <= threshold)
            })
            .collect();
        for threads in [1, 4] {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .unwrap();
            let actual = pool.install(|| voxelize_surface(&grid, &triangles, Control::none()));
            assert_eq!(actual, expected, "surface differs with {threads} workers");
        }
    }
    assert!(
        voxelize_surface(&grid, &[], Control::none())
            .iter()
            .all(|cell| !cell)
    );
}

fn count_edge_connected_components(mesh: &IndexedMesh) -> usize {
    let topo = Topology::build(mesh);
    let n = mesh.triangles.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    for info in topo.edges.values() {
        let mut faces = info.faces.iter();
        if let Some(&first) = faces.next() {
            let root = find(&mut parent, first as usize);
            for &other in faces {
                let other_root = find(&mut parent, other as usize);
                parent[other_root] = root;
            }
        }
    }
    let mut roots = std::collections::HashSet::new();
    for i in 0..n {
        roots.insert(find(&mut parent, i));
    }
    roots.len()
}

/// A smoothed cavity generated from a clean closed source mesh must itself be
/// a single watertight surface (DragonFruit's 2026-07-12 audit regression).
#[test]
fn smoothed_cavity_from_sphere_is_single_watertight_component() {
    let sphere = icosphere(20.0, 3);
    let options = HollowOptions {
        mode: HollowMode::Cavity,
        voxel_resolution: 64,
        shell_thickness_mm: 1.2,
        smooth_internal_surfaces: true,
        preview_cavity_only: true,
        ..HollowOptions::default()
    };

    let outcome = hollow_voxel(sphere, &options);
    let cavity = &outcome.mesh; // preview_cavity_only: the out mesh IS the cavity
    assert!(
        cavity.triangle_count() > 1000,
        "got {} triangles",
        cavity.triangle_count()
    );

    let topo = Topology::build(cavity);
    assert_eq!(topo.boundary_edges().len(), 0, "open shard fragments");
    assert_eq!(count_edge_connected_components(cavity), 1);
}

#[test]
fn visual_cavity_matches_mask_without_finalizing_or_cloning_exterior() {
    let source = icosphere(20.0, 2);
    let session = HollowSession::new(source.clone(), 48);
    let mut options = HollowOptions {
        voxel_resolution: 48,
        shell_thickness_mm: 2.0,
        preview_cavity_only: true,
        preview_voxel_spheres: true,
        ..HollowOptions::default()
    };
    let voxels = session.run(&options);
    options.preview_voxel_spheres = false;
    options.preview_surface_only = true;
    let visual = session.run(&options);
    assert_eq!(visual.removed_voxel_indices, voxels.removed_voxel_indices);
    assert_eq!(visual.removed_voxel_centers, voxels.removed_voxel_centers);
    assert_eq!(visual.report.removed_voxels, voxels.report.removed_voxels);
    assert!(!visual.mesh.triangles.is_empty());
    assert!(
        visual.cavity_mesh.is_none(),
        "preview exports one surface, not a duplicate"
    );
    assert!(
        visual.mesh.signed_volume() < 0.0,
        "cavity must face inward for subtractive stencil caps"
    );
    // Direct and cached-session routes implement the same visual contract.
    let direct = hollow_voxel(source, &options);
    assert_eq!(direct.mesh.positions.len(), visual.mesh.positions.len());
    assert_eq!(direct.removed_voxel_indices, visual.removed_voxel_indices);
    let selected = visual.removed_voxel_indices[0] as usize;
    options.blocked_voxel_indices = vec![selected];
    let blocked = session.run(&options);
    assert!(!blocked.removed_voxel_indices.contains(&(selected as u32)));
    assert!(blocked.blocked_voxel_indices.contains(&(selected as u32)));
}

#[test]
fn weld_gate_classifies_defect_classes() {
    let tet = IndexedMesh::new(
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ],
        vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]],
    );
    let clean = summarize_mesh_defects(&tet);
    assert_eq!(clean.boundary_edges, 0);
    assert_eq!(clean.non_manifold_edges, 0);
    assert_eq!(clean.inconsistent_edges, 0);
    assert!(
        !weld_retries_worthwhile(&clean),
        "no defects -> nothing for welding to fix"
    );

    let open = IndexedMesh::new(tet.positions.clone(), vec![[0, 2, 1], [0, 1, 3], [1, 2, 3]]);
    let open_defects = summarize_mesh_defects(&open);
    assert!(open_defects.boundary_edges > 0);
    assert_eq!(open_defects.non_manifold_edges, 0);
    assert!(
        weld_retries_worthwhile(&open_defects),
        "boundary-only defects are weld-worthy"
    );

    let mut non_manifold = tet.clone();
    non_manifold.positions.push(Vec3::new(1.0, 1.0, 1.0));
    non_manifold.triangles.push([0, 1, 4]);
    let nm_defects = summarize_mesh_defects(&non_manifold);
    assert!(nm_defects.non_manifold_edges > 0);
    assert!(
        !weld_retries_worthwhile(&nm_defects),
        "non-manifold edges are not fixable by welding"
    );
}

#[test]
fn normalize_with_weld_compacts_orphans_and_preserves_triangles() {
    let positions = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(10.0, 0.0, 0.0),
        Vec3::new(0.0, 10.0, 0.0),
        Vec3::new(0.0, 0.0, 20.0),
        Vec3::new(10.0, 0.0, 20.0),
        Vec3::new(0.0, 10.0, 20.0),
        // Degenerate triangle: three collinear points far from the rest.
        Vec3::new(50.0, 50.0, 50.0),
        Vec3::new(51.0, 50.0, 50.0),
        Vec3::new(52.0, 50.0, 50.0),
    ];
    let mesh = IndexedMesh::new(positions, vec![[0u32, 1, 2], [3, 4, 5], [6, 7, 8]]);

    let normalized = normalize_mesh_for_boolean_with_weld(mesh, 1e-6);

    assert_eq!(
        normalized.triangles.len(),
        2,
        "degenerate triangle should be removed"
    );
    let mut used = vec![false; normalized.positions.len()];
    for tri in &normalized.triangles {
        for &v in tri {
            assert!((v as usize) < normalized.positions.len());
            used[v as usize] = true;
        }
    }
    assert!(
        used.iter().all(|&u| u),
        "normalization left unreferenced vertices behind"
    );
}

#[test]
fn polygonizer_emits_nothing_for_label_only_crossings() {
    let grid = GridSpec {
        nx: 2,
        ny: 2,
        nz: 2,
        voxel_mm: 1.0,
        min: Vec3::new(0.0, 0.0, 0.0),
    };
    // Hard labels split 4/4 along x, but the scalar field is uniformly positive.
    let mut positive = vec![false; 8];
    let mut negative = vec![false; 8];
    let scalar = vec![0.5f32; 8];
    for i in 0..8 {
        let (x, _, _) = grid.coords(i);
        if x == 0 {
            positive[i] = true;
        } else {
            negative[i] = true;
        }
    }
    let mesh = organic_boundary_mesh(&grid, &positive, &negative, &scalar, false);
    assert_eq!(mesh.triangle_count(), 0);
}

#[test]
fn scalar_field_blur_cannot_flip_outer_skin_voxels() {
    let grid = GridSpec {
        nx: 5,
        ny: 5,
        nz: 5,
        voxel_mm: 1.0,
        min: Vec3::new(0.0, 0.0, 0.0),
    };
    let n = grid.cell_count();
    let mut solid = vec![false; n];
    let mut keep = vec![false; n];
    let mut dist = vec![0.0f32; n];
    for z in 0..5 {
        for y in 0..5 {
            for x in 1..=3 {
                let i = grid.idx(x, y, z);
                solid[i] = true;
                if x == 1 {
                    keep[i] = true;
                    dist[i] = 0.0;
                } else {
                    dist[i] = 25.0;
                }
            }
        }
    }
    let field = build_smoothed_cavity_scalar_field(&grid, &solid, &keep, &dist, 1.0, 9, false);
    for z in 0..5 {
        for y in 0..5 {
            let i = grid.idx(1, y, z);
            assert!(
                field[i] >= 0.0,
                "outer-skin voxel (1,{y},{z}) blurred negative: {}",
                field[i]
            );
        }
    }
}

#[test]
fn parity_refinement_clears_enclosed_non_surface_cavity_component() {
    let grid = GridSpec {
        nx: 5,
        ny: 5,
        nz: 5,
        voxel_mm: 1.0,
        min: Vec3::new(0.0, 0.0, 0.0),
    };
    let mut solid = vec![false; grid.cell_count()];
    let mut surface = vec![false; solid.len()];
    for z in 1..=3 {
        for y in 1..=3 {
            for x in 1..=3 {
                let i = grid.idx(x, y, z);
                solid[i] = true;
                surface[i] = x == 1 || x == 3 || y == 1 || y == 3 || z == 1 || z == 3;
            }
        }
    }
    let cavity_index = grid.idx(2, 2, 2);
    let mesh = merged(&[cube(1.0, 4.0, false), cube(2.0, 3.0, true)]);
    refine_non_surface_solid_components_with_parity(
        &grid,
        &surface,
        &mut solid,
        &mesh,
        &Bvh::build(&mesh),
        Control::none(),
    )
    .unwrap();
    assert!(
        !solid[cavity_index],
        "parity refinement should clear the enclosed cavity center"
    );
}

#[test]
fn removed_voxel_collectors_emit_boundary_only_not_full_interior() {
    let grid = GridSpec {
        nx: 7,
        ny: 7,
        nz: 7,
        voxel_mm: 1.0,
        min: Vec3::new(0.0, 0.0, 0.0),
    };
    let mut solid = vec![false; grid.cell_count()];
    let mut keep = vec![false; solid.len()];
    for z in 1..=5 {
        for y in 1..=5 {
            for x in 1..=5 {
                let i = grid.idx(x, y, z);
                solid[i] = true;
                keep[i] = x == 1 || x == 5 || y == 1 || y == 5 || z == 1 || z == 5;
            }
        }
    }
    let center_index = grid.idx(3, 3, 3);
    let boundary_index = grid.idx(2, 2, 2);
    let indices = collect_removed_voxel_indices(&grid, &solid, &keep);
    assert_eq!(indices.len(), 26);
    assert!(!indices.contains(&(center_index as u32)));
    assert!(indices.contains(&(boundary_index as u32)));
    assert_eq!(
        collect_removed_voxel_centers(&grid, &solid, &keep).len(),
        26 * 3
    );
}

#[test]
fn lasso_selection_returns_full_through_depth_cavity_column_not_just_boundary() {
    // 5x5x5 block with a kept 1-voxel shell; the column x = y = 2 crosses
    // three removed voxels (z = 1..=3), including the occluded center.
    let grid = GridSpec {
        nx: 5,
        ny: 5,
        nz: 5,
        voxel_mm: 1.0,
        min: Vec3::new(0.0, 0.0, 0.0),
    };
    let solid = vec![true; grid.cell_count()];
    let keep: Vec<bool> = (0..grid.cell_count())
        .map(|i| {
            let (x, y, z) = grid.coords(i);
            [x, y, z].iter().any(|&c| c == 0 || c == 4)
        })
        .collect();
    let identity = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    // Orthographic top view: x, y in [0, 5] to NDC [-1, 1]; z into [-1, 1].
    let view_proj = [
        0.4, 0.0, 0.0, 0.0, 0.0, 0.4, 0.0, 0.0, 0.0, 0.0, 0.4, 0.0, -1.0, -1.0, -1.0, 1.0,
    ];
    let rect = [500.0, 500.0];
    // Pixel square around voxel column (2, 2): x in [200, 300], y in [200, 300].
    let polygon = [
        [210.0, 210.0],
        [290.0, 210.0],
        [290.0, 290.0],
        [210.0, 290.0],
    ];
    let selected = select_removed_voxels_in_polygon(
        &grid, &solid, &keep, &polygon, &identity, &view_proj, rect,
    );
    let mut expected: Vec<u32> = (1..=3).map(|z| grid.idx(2, 2, z) as u32).collect();
    expected.sort_unstable();
    let mut got = selected.clone();
    got.sort_unstable();
    assert_eq!(got, expected);
    assert!(
        got.contains(&(grid.idx(2, 2, 2) as u32)),
        "occluded center voxel is selectable"
    );
}

#[test]
fn blocked_voxel_collector_drops_stale_indices_and_stays_positionally_aligned() {
    let grid = GridSpec {
        nx: 3,
        ny: 3,
        nz: 3,
        voxel_mm: 1.0,
        min: Vec3::new(0.0, 0.0, 0.0),
    };
    let mut solid = vec![false; grid.cell_count()];
    let first = grid.idx(1, 1, 1);
    let second = grid.idx(2, 1, 1);
    solid[first] = true;
    solid[second] = true;
    let not_solid = grid.idx(0, 0, 0);
    let out_of_bounds = solid.len() + 4;
    let blocked = vec![first, out_of_bounds, not_solid, second];
    let (centers, accepted) = collect_blocked_voxel_data(&grid, &solid, &blocked);
    assert_eq!(accepted, vec![first as u32, second as u32]);
    assert_eq!(centers.len(), accepted.len() * 3);
    let expected = grid.center_world(2, 1, 1);
    assert_eq!(&centers[3..6], &[expected.x, expected.y, expected.z]);
}

#[test]
fn source_void_separator_voxel_is_preserved() {
    let grid = GridSpec {
        nx: 3,
        ny: 3,
        nz: 3,
        voxel_mm: 1.0,
        min: Vec3::new(0.0, 0.0, 0.0),
    };
    let mut solid = vec![true; grid.cell_count()];
    let left_void = grid.idx(0, 1, 1);
    let right_void = grid.idx(2, 1, 1);
    let separator = grid.idx(1, 1, 1);
    solid[left_void] = false;
    solid[right_void] = false;
    let void_components = label_void_components(&grid, &solid);
    assert_ne!(void_components[left_void], void_components[right_void]);
    let mut keep = solid.clone();
    keep[separator] = false;
    preserve_source_void_separators(&grid, &solid, &void_components, &mut keep);
    assert!(keep[separator]);
}

#[test]
fn thin_shells_disable_chamfering_until_there_is_enough_slack() {
    assert_eq!(effective_internal_cavity_chamfer_passes(1.0, 4.0, 2), 0);
    assert_eq!(effective_internal_cavity_chamfer_passes(2.0, 2.4, 2), 0);
    assert_eq!(effective_internal_cavity_chamfer_passes(2.0, 2.5, 2), 1);
    assert_eq!(effective_internal_cavity_chamfer_passes(2.0, 3.9, 2), 1);
    assert_eq!(effective_internal_cavity_chamfer_passes(2.0, 4.0, 2), 2);
    assert_eq!(effective_internal_cavity_chamfer_passes(2.0, 4.0, 1), 1);
}

#[test]
fn thin_shells_use_reduced_internal_smoothing_until_there_is_enough_slack() {
    let thin = effective_internal_cavity_smoothing_profile(1.0, true, 2.4);
    assert_eq!(thin.scalar_field_blur_iterations, 3);
    assert_eq!(thin.taubin_iterations, 8);
    assert!(thin.taubin_max_step_scale < 0.42);
    let thick = effective_internal_cavity_smoothing_profile(2.0, true, 4.0);
    assert_eq!(thick.scalar_field_blur_iterations, 9);
    assert_eq!(thick.taubin_iterations, 18);
    assert!((thick.taubin_max_step_scale - 0.62).abs() < 1e-5);
    let disabled = effective_internal_cavity_smoothing_profile(2.0, false, 4.0);
    assert_eq!(
        (
            disabled.scalar_field_blur_iterations,
            disabled.taubin_iterations
        ),
        (0, 0)
    );
}

#[test]
fn blocked_kept_voxels_stay_positive_in_cavity_scalar_field() {
    let grid = GridSpec {
        nx: 3,
        ny: 1,
        nz: 1,
        voxel_mm: 1.0,
        min: Vec3::new(0.0, 0.0, 0.0),
    };
    let field = build_smoothed_cavity_scalar_field(
        &grid,
        &[true, true, true],
        &[true, false, true],
        &[0.5, 1.5, 4.5],
        1.0,
        0,
        false,
    );
    assert!(field[0] > 0.0);
    assert!(field[1] < 0.0);
    assert!(
        field[2] > 0.0,
        "blocked kept voxels deep in the cavity should remain positive"
    );
}

#[test]
fn cavity_polygonizer_uses_scalar_sign_not_only_hard_voxel_labels() {
    let positions = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Vec3::new(1.0, 0.0, 1.0),
        Vec3::new(1.0, 1.0, 1.0),
        Vec3::new(0.0, 1.0, 1.0),
    ];
    let scalar = [1.0, -1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
    let mut soup = Vec::new();
    polygonize_cavity_tetrahedron(
        &mut soup,
        [0, 5, 1, 6],
        &positions,
        &scalar,
        &[true; 8],
        &[false; 8],
        false,
    );
    assert_eq!(soup.len(), 9);
}

#[test]
fn cavity_micro_repair_can_weld_a_tiny_near_seam_before_boolean() {
    let mesh = IndexedMesh::new(
        vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.00002, 0.0, 1.00001),
        ],
        vec![[0, 1, 2], [0, 3, 1], [1, 4, 2], [2, 3, 0]],
    );
    assert!(!Topology::build(&mesh).boundary_edges().is_empty());
    let repaired = stabilize_cavity_mesh_for_boolean(mesh, 1.0);
    let after = Topology::build(&repaired);
    assert_eq!(after.boundary_edges().len(), 0);
    assert_eq!(after.non_manifold_edges().len(), 0);
}

#[test]
fn infill_mode_keeps_more_material_than_plain_cavity() {
    let mesh = cube(0.0, 10.0, false);
    let cavity_options = HollowOptions {
        mode: HollowMode::Cavity,
        voxel_resolution: 64,
        shell_thickness_mm: 1.6,
        smooth_internal_surfaces: false,
        internal_chamfer_passes: 0,
        ..HollowOptions::default()
    };
    let infill_options = HollowOptions {
        mode: HollowMode::Infill,
        ..cavity_options.clone()
    };
    let cavity = hollow_voxel(mesh.clone(), &cavity_options);
    let infill = hollow_voxel(mesh, &infill_options);
    assert_eq!(infill.report.removed_voxels, cavity.report.removed_voxels);
    assert!(infill.mesh.triangle_count() > cavity.mesh.triangle_count());
}

// SoulCrafted additions.

#[test]
fn the_exact_cavity_of_a_sphere_is_closed_and_faces_inward() {
    let session = HollowSession::new(icosphere(20.0, 3), 64);
    let options = HollowOptions {
        shell_thickness_mm: 2.0,
        exact_wall: true,
        ..HollowOptions::default()
    };
    let cavity = session
        .exact_cavity(&options, &session.dist, Control::none())
        .unwrap();
    assert!(cavity.closed);
    assert!(is_edge_manifold(&cavity.wall));
    assert!(cavity.infill.is_none());
    let source = session.source_mesh().signed_volume();
    let inside = -cavity.wall.signed_volume();
    assert!(
        inside > source * 0.5 && inside < source,
        "source {source}, cavity {inside}"
    );
    assert!(cavity.removed_voxels > 0);
}

#[test]
fn a_cancelled_session_stops() {
    let token = crate::control::CancelToken::new();
    token.cancel();
    let result = HollowSession::build(
        icosphere(10.0, 2),
        64,
        [0.0, 0.0, 0.0, 1.0],
        None,
        Control::new(Some(&token), None),
    );
    assert!(result.is_err());
}

#[test]
fn rotated_session_outputs_come_back_unrotated() {
    let q = [0.0, 0.382_683_43, 0.0, 0.923_879_5];
    let source = icosphere(15.0, 2);
    let options = HollowOptions {
        preview_cavity_only: true,
        preview_surface_only: true,
        rotation_quat: q,
        ..HollowOptions::default()
    };
    let out = hollow_voxel(source.clone(), &options);
    // The cavity of a sphere centered at the origin stays centered after the
    // round trip.
    let bbox = out.mesh.bbox();
    let c = bbox.center();
    assert!(c.length() < 1.0, "center {c:?}");
    assert!(bbox.max.x < 15.0 && bbox.min.x > -15.0);
}

/// Over exact distances the smoothing never removes material, adds at most
/// its cap, and never turns a cavity voxel into material (#266).
#[test]
fn clamped_smoothing_stays_between_the_exact_field_and_its_cap() {
    // Material: may rise by the cap, never drop.
    assert_eq!(clamp_smoothed(5.0, 1.0, 0.5), 1.5);
    assert_eq!(clamp_smoothed(0.2, 1.0, 0.5), 1.0);
    assert_eq!(clamp_smoothed(1.2, 1.0, 0.5), 1.2);
    // Cavity: may rise by the cap, but stays cavity.
    assert_eq!(clamp_smoothed(-2.0, -3.0, 0.5), -2.5);
    assert_eq!(clamp_smoothed(0.4, -0.3, 0.5), -EXACT_WALL_SAMPLE_MARGIN);
    assert_eq!(clamp_smoothed(-4.0, -3.0, 0.5), -3.0);
    // A cavity value already within the margin is left where it is.
    assert_eq!(clamp_smoothed(1.0, -0.01, 0.5), -0.01);
}

/// A thin channel between two cavities: the clamped blur leaves the field's
/// signs, and so the cavity's voxels, as they were.
#[test]
fn clamped_smoothing_keeps_every_voxel_on_its_side() {
    let grid = GridSpec {
        nx: 9,
        ny: 9,
        nz: 9,
        voxel_mm: 1.0,
        min: Vec3::new(0.0, 0.0, 0.0),
    };
    let n = grid.cell_count();
    let mut solid = vec![false; n];
    let mut keep = vec![false; n];
    let mut dist = vec![0.0f32; n];
    for z in 1..8 {
        for y in 1..8 {
            for x in 1..8 {
                let i = grid.idx(x, y, z);
                solid[i] = true;
                // A one-voxel channel along x through the middle.
                let edge = x.min(8 - x).min(y.min(8 - y)).min(z.min(8 - z)) as f32;
                dist[i] = if y == 4 && z == 4 { 1.4 } else { edge - 1.0 };
                keep[i] = dist[i] <= 1.0;
            }
        }
    }
    let exact = build_smoothed_cavity_scalar_field(&grid, &solid, &keep, &dist, 1.0, 0, true);
    let smoothed = build_smoothed_cavity_scalar_field(&grid, &solid, &keep, &dist, 1.0, 9, true);
    for i in 0..n {
        if !solid[i] {
            continue;
        }
        assert_eq!(smoothed[i] >= 0.0, keep[i], "voxel {i} changed side");
        assert!(smoothed[i] >= exact[i], "voxel {i} lost material");
        assert!(
            smoothed[i] <= exact[i] + EXACT_SMOOTHING_CAP_VOXELS + 1e-6,
            "voxel {i} rose past the cap"
        );
    }
}

/// The pieces found on every core (#451) are the ones the flood fill and
/// the parity walk found: the same solid voxels, the same void pieces.
#[test]
fn the_pieces_found_on_every_core_are_the_flood_fills() {
    let hollow_box = merged(&[cube(-10.0, 10.0, false), cube(-6.0, 6.0, true)]);
    let island_in_a_hollow = merged(&[
        cube(-10.0, 10.0, false),
        cube(-6.0, 6.0, true),
        cube(-2.0, 2.0, false),
    ]);
    let apart = merged(&[cube(-10.0, -4.0, false), cube(4.0, 10.0, false)]);
    for (name, mesh) in [
        ("sphere", icosphere(8.0, 3)),
        ("hollow box", hollow_box),
        ("island in a hollow", island_in_a_hollow),
        ("two boxes", apart),
    ] {
        for voxel in [0.45, 0.8, 1.3] {
            let grid = GridSpec::for_bbox_with_voxel(&mesh.bbox(), voxel);
            let triangles: Vec<_> = mesh
                .triangles
                .iter()
                .map(|t| {
                    let [a, b, c] = t.map(|v| mesh.positions[v as usize]);
                    TriangleCache::from_points(a, b, c)
                })
                .collect();
            let surface = voxelize_surface(&grid, &triangles, Control::none());
            let bvh = Bvh::build(&mesh);
            let (solid, labels) =
                classify_non_surface_pieces(&grid, &surface, &mesh, &bvh, Control::none()).unwrap();
            let (expected, expected_labels) =
                classify_by_flood_fill(&grid, &surface, &mesh, &bvh, Control::none()).unwrap();
            assert_eq!(solid, expected, "{name} at {voxel} mm: solid voxels");
            let mut to_old = std::collections::HashMap::new();
            let mut to_new = std::collections::HashMap::new();
            for (new, old) in labels.iter().zip(&expected_labels) {
                assert_eq!(
                    *to_old.entry(*new).or_insert(*old),
                    *old,
                    "{name}: a piece split"
                );
                assert_eq!(
                    *to_new.entry(*old).or_insert(*new),
                    *new,
                    "{name}: pieces merged"
                );
            }
            if name != "sphere" && name != "two boxes" {
                assert!(
                    to_old.len() > 2,
                    "{name}: the sealed pocket is its own piece"
                );
            }
        }
    }
}
