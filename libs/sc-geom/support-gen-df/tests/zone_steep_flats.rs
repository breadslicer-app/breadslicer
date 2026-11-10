//! A large flat face stood near upright, under the zone rules (SoulCrafted,
//! #500).
//!
//! The underside of a mini's base, stood 12 degrees past vertical: the
//! steep-flat pass takes it for an overhang and the zone rules gave it the
//! overhang lattice, though it prints without a tip. The zone rules give each
//! down-facing face its rule by angle, and a face that steep takes none.

use support_gen_df::native::overhang::{OverhangScan, overhang_and_stability_from_soup};
use support_gen_df::pipeline::without_steep_flats;

/// The 12 triangles of the box `[0, 20] x [0, 20] x [0, 2]`, each face
/// looking out, turned `deg` about x and lifted 30 mm.
fn turned_slab(deg: f32) -> Vec<f32> {
    let c = [
        [0.0, 0.0, 0.0],
        [20.0, 0.0, 0.0],
        [20.0, 20.0, 0.0],
        [0.0, 20.0, 0.0],
        [0.0, 0.0, 2.0],
        [20.0, 0.0, 2.0],
        [20.0, 20.0, 2.0],
        [0.0, 20.0, 2.0],
    ];
    let faces = [
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    let (s, k) = deg.to_radians().sin_cos();
    let turn = |[x, y, z]: [f32; 3]| [x, y * k - z * s, y * s + z * k + 30.0];
    faces
        .iter()
        .flat_map(|f| f.map(|i| turn(c[i])))
        .flatten()
        .collect()
}

fn scan(soup: &[f32], angle: f32) -> OverhangScan {
    let (regions, stability) = overhang_and_stability_from_soup(soup, angle, 0.25, false);
    OverhangScan {
        regions,
        stability,
        steep_regions: Vec::new(),
    }
}

#[test]
fn a_flat_face_near_upright_is_no_overhang_of_the_zone_rules() {
    // The slab's underside looks 12 degrees down from level: 78 from
    // horizontal, past the overhang angle (38) and the stabilization's (75).
    let soup = turned_slab(78.0);
    for angle in [38.0, 75.0] {
        let found = scan(&soup, angle);
        let flats = found.regions.iter().filter(|r| r.steep_flat).count();
        assert_eq!(
            flats, 1,
            "the steep-flat pass takes the underside at {angle}"
        );
        let kept = without_steep_flats(found);
        assert!(kept.regions.iter().all(|r| !r.steep_flat), "at {angle}");
        // The slab's low end does look down: it stays an overhang.
        assert_eq!(kept.regions.len(), 1, "the low end at {angle}");
        let end = &kept.regions[0];
        assert!(end.normal[2] < -0.9, "{:?}", end.normal);
    }
}
