use crate::*;

/// A welded mesh from triangles given by their corners.
fn mesh(triangles: &[[[f32; 3]; 3]]) -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut faces = Vec::new();
    for t in triangles {
        faces.push(t.map(|p| {
            positions.iter().position(|q| *q == p).unwrap_or_else(|| {
                positions.push(p);
                positions.len() - 1
            }) as u32
        }));
    }
    (positions, faces)
}

/// A box from `lo` to `hi`, facing out.
fn cuboid(lo: [f32; 3], hi: [f32; 3]) -> Vec<[[f32; 3]; 3]> {
    let c = |i: usize| {
        [
            if i & 1 == 0 { lo[0] } else { hi[0] },
            if i & 2 == 0 { lo[1] } else { hi[1] },
            if i & 4 == 0 { lo[2] } else { hi[2] },
        ]
    };
    [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ]
    .iter()
    .flat_map(|q| [[c(q[0]), c(q[1]), c(q[2])], [c(q[0]), c(q[2]), c(q[3])]])
    .collect()
}

/// Turned about X by `deg`.
fn turned(triangles: &[[[f32; 3]; 3]], deg: f32) -> Vec<[[f32; 3]; 3]> {
    let (s, c) = deg.to_radians().sin_cos();
    triangles
        .iter()
        .map(|t| t.map(|p| [p[0], c * p[1] - s * p[2], s * p[1] + c * p[2]]))
        .collect()
}

fn regions_of(triangles: &[[[f32; 3]; 3]], angle: f32) -> Vec<OverhangRegion> {
    let (positions, faces) = mesh(triangles);
    overhang_regions(
        &Surface {
            positions: &positions,
            triangles: &faces,
        },
        angle,
        0.25,
    )
}

#[test]
fn a_lifted_box_overhangs_by_its_underside_alone() {
    let regions = regions_of(&cuboid([0.0, 0.0, 5.0], [10.0, 10.0, 15.0]), 45.0);
    assert_eq!(regions.len(), 1);
    let r = &regions[0];
    assert_eq!(r.triangle_ids.len(), 2);
    assert!((r.area_mm2 - 100.0).abs() < 1e-3 && (r.projected_area_mm2 - 100.0).abs() < 1e-3);
    assert!(r.angle_deg.abs() < 1e-3);
    assert_eq!(r.normal, [0.0, 0.0, -1.0]);
    // The shadow, 40 pixels a side plus the half pixel around, at the
    // underside's height.
    let f = &r.footprint;
    assert_eq!((f.width, f.height), (40, 40));
    assert!(f.data.iter().all(|&d| d == 1));
    assert!(f.surface_z.iter().all(|&z| (z - 5.0).abs() < 1e-4));
    // One outline, drawn in a quarter millimetre.
    assert_eq!(r.perimeter_loops.len(), 1);
    for p in &r.perimeter_loops[0] {
        assert!(
            (0.17..=9.83).contains(&p[0]) && (0.17..=9.83).contains(&p[1]),
            "{p:?}"
        );
    }
    // It does not lean: no drag.
    assert_eq!(r.drag_moment_mm3, 0.0);
}

#[test]
fn a_box_turned_past_the_angle_overhangs_by_its_lower_face() {
    // Turned 30°: the underside is 30° from horizontal, the side 60°.
    let regions = regions_of(&turned(&cuboid([0.0; 3], [10.0; 3]), 30.0), 45.0);
    assert_eq!(regions.len(), 1, "{regions:?}");
    assert!((regions[0].angle_deg - 30.0).abs() < 1e-3);
    assert!(regions[0].drag_moment_mm3 > 0.0);
}

#[test]
fn two_parts_are_two_regions_in_the_order_of_their_first_triangle() {
    let mut both = cuboid([20.0, 0.0, 5.0], [30.0, 10.0, 15.0]);
    both.extend(cuboid([0.0, 0.0, 2.0], [4.0, 4.0, 6.0]));
    let regions = regions_of(&both, 45.0);
    assert_eq!(regions.len(), 2);
    assert!(regions[0].xy_min[0] >= 20.0 && regions[1].xy_max[0] <= 4.0);
}

/// A large steep face counts, a small one does not, and a crease splits two
/// faces that are large only together.
#[test]
fn steep_faces_are_large_flat_faces_past_the_angle() {
    // A slab leaning so its underside is 60° from horizontal, 20 × 20.
    let slab = turned(&cuboid([0.0, 0.0, 0.0], [20.0, 1.0, 20.0]), -30.0);
    let regions = regions_of(&slab, 45.0);
    assert!(
        regions
            .iter()
            .any(|r| r.steep_flat && (r.area_mm2 - 400.0).abs() < 1e-2),
        "{regions:?}"
    );

    let small = turned(&cuboid([0.0, 0.0, 0.0], [5.0, 1.0, 5.0]), -30.0);
    assert!(regions_of(&small, 45.0).iter().all(|r| !r.steep_flat));
}

#[test]
fn a_raft_stands_the_part_on_its_shadow() {
    // A box standing on a thin foot at one side: on its own it stands on the
    // foot, on a raft on its whole shadow.
    let mut part = cuboid([0.0, 0.0, 0.0], [2.0, 10.0, 1.0]);
    part.extend(cuboid([0.0, 0.0, 3.0], [20.0, 10.0, 10.0]));
    let (positions, faces) = mesh(&part);
    let surface = Surface {
        positions: &positions,
        triangles: &faces,
    };
    let alone = stability(&surface, false).unwrap();
    let raft = stability(&surface, true).unwrap();
    assert!((alone.bearing_area_mm2 - 20.0).abs() < 1e-3, "{alone:?}");
    assert!((raft.bearing_area_mm2 - 200.0).abs() < 1e-3, "{raft:?}");
    // Nothing leans, so nothing drags it over.
    assert_eq!(raft.drag_moment_mm3, 0.0);
    assert!(raft.adhesion_ratio.is_infinite());
}

#[test]
fn nothing_has_no_report_and_no_regions() {
    let surface = Surface {
        positions: &[],
        triangles: &[],
    };
    assert!(stability(&surface, false).is_none());
    assert!(overhang_regions(&surface, 45.0, 0.25).is_empty());
}
