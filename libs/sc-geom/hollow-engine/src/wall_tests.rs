use super::*;
use crate::bvh::point_triangle_distance;
use crate::mesh::{IndexedMesh, Vec3};
use crate::shapes::{icosphere, merged, tessellated_box};

fn session(mesh: IndexedMesh, voxel_mm: f32) -> Volume {
    Volume::build(mesh, voxel_mm, Control::none()).unwrap()
}

/// A 40 mm cube turned 30° about Z and 20° about X: its faces follow no
/// grid axis.
fn turned_cube() -> IndexedMesh {
    let mut cube = tessellated_box([-20.0; 3], [20.0; 3], 8);
    let (z, x) = (30f32.to_radians(), 20f32.to_radians());
    for p in &mut cube.positions {
        let (px, py) = (p.x * z.cos() - p.y * z.sin(), p.x * z.sin() + p.y * z.cos());
        let (py, pz) = (py * x.cos() - p.z * x.sin(), py * x.sin() + p.z * x.cos());
        *p = Vec3::new(px, py, pz);
    }
    cube
}

/// A block with a 3 mm fin: thin walls and a concave corner.
fn finned_block() -> IndexedMesh {
    merged(&[
        tessellated_box([-15.0, -10.0, -10.0], [15.0, 10.0, 10.0], 6),
        tessellated_box([15.0, -1.5, -8.0], [27.0, 1.5, 8.0], 6),
    ])
}

/// Exact distance of `p` to `mesh`, over every triangle.
fn brute_distance(mesh: &IndexedMesh, p: Vec3) -> f32 {
    (0..mesh.triangle_count() as u32)
        .map(|f| {
            let [a, b, c] = mesh.tri_positions(f);
            point_triangle_distance(p, a, b, c)
        })
        .fold(f32::INFINITY, f32::min)
}

#[test]
fn the_nearest_triangle_matches_a_full_scan() {
    let mesh = finned_block();
    let session = session(mesh.clone(), 0.65);
    let bvh = session.source_bvh();
    let grid = session.grid();
    for i in (0..grid.cell_count()).step_by(97) {
        let (x, y, z) = grid.coords(i);
        let p = grid.center_world(x, y, z);
        let expected = brute_distance(&mesh, p);
        let found = bvh.nearest_distance(&mesh, p, 1e6).unwrap();
        assert!(
            (found - expected).abs() < 1e-5,
            "{p:?}: {found} vs {expected}"
        );
        if expected > 0.5 {
            assert_eq!(bvh.nearest_distance(&mesh, p, expected - 0.1), None);
        }
    }
}

/// Every voxel within the band holds its exact distance, and none outside it
/// reads closer than the band: the seed distance misses no voxel.
#[test]
fn every_voxel_within_the_band_is_exact() {
    for (name, mesh, voxel) in [
        ("sphere", icosphere(20.0, 3), 0.65),
        ("turned cube", turned_cube(), 0.65),
        ("finned block", finned_block(), 0.4),
    ] {
        let session = session(mesh.clone(), voxel);
        let band = 2.0 / voxel + 5.0;
        let walls = WallDistances::measure(&session, band, None, Control::none()).unwrap();
        let grid = session.grid();
        let solid = session.solid_mask();
        let mut exact = 0;
        for i in 0..grid.cell_count() {
            if !solid[i] {
                continue;
            }
            let (x, y, z) = grid.coords(i);
            let truth = brute_distance(&mesh, grid.center_world(x, y, z)) / voxel;
            let got = walls.dist[i];
            if truth <= band {
                assert!(
                    (got - truth).abs() < 1e-4,
                    "{name}: voxel {i}: {got} vs {truth}"
                );
                exact += 1;
            } else {
                assert!(got >= band, "{name}: voxel {i} beyond the band reads {got}");
            }
        }
        assert!(exact > 1000, "{name}: {exact}");
    }
}

/// A wall reads the same distances whether a wider band was measured before
/// it or not, and a wider band reuses a narrower one.
#[test]
fn a_wider_band_reads_the_same_when_clamped() {
    let session = session(finned_block(), 0.65);
    let narrow = WallDistances::measure(&session, 6.0, None, Control::none()).unwrap();
    let wide = WallDistances::measure(&session, 11.0, None, Control::none()).unwrap();
    let grown = WallDistances::measure(&session, 11.0, Some(&narrow), Control::none()).unwrap();
    let bits = |v: Vec<f32>| v.into_iter().map(f32::to_bits).collect::<Vec<_>>();
    assert_eq!(bits(narrow.clamped(6.0)), bits(wide.clamped(6.0)));
    assert_eq!(bits(wide.clamped(11.0)), bits(grown.clamped(11.0)));
    assert_eq!(bits(narrow.clamped(4.5)), bits(grown.clamped(4.5)));
}

#[test]
fn measuring_stops_when_cancelled() {
    let session = session(icosphere(20.0, 3), 0.65);
    let token = crate::control::CancelToken::new();
    token.cancel();
    let result = WallDistances::measure(&session, 8.0, None, Control::new(Some(&token), None));
    assert!(matches!(result, Err(Cancelled)));
}
