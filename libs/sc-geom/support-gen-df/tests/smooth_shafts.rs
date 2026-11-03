//! SoulCrafted's smooth shaft mesher: tubes are closed and outward-wound,
//! bulge into a bicone at joints and nowhere else, and on DragonFruit's own
//! support states they replace the joint spheres without opening the soup
//! any further.
//! DragonFruit's style stays the default (its parity is
//! `df_parity_support_triangles`).

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use support_gen_df::features::slicing::smooth_shafts::{
    ChainPoint, ShaftStyle, SmoothShaftParams, append_smooth_chain,
};
use support_gen_df::features::slicing::support_world_triangles::{
    SupportSliceInput, TriangleSink, build_support_and_raft_world_triangles,
};
use support_gen_df::supports::rafts::crenelated::raft_types::{
    DEFAULT_RAFT_SETTINGS, RaftBottomMode, RaftSettings,
};
use support_gen_df::supports::types::{SupportCollections, Vec3};

/// Each undirected edge of the welded soup and how many triangles use it,
/// plus the signed volume.
/// An undirected edge, as its two endpoints' bit patterns.
type Edge = ([u32; 3], [u32; 3]);

fn topology(soup: &[f32]) -> (HashMap<Edge, usize>, f64) {
    let mut edges = HashMap::new();
    let mut volume = 0.0;
    for t in soup.as_chunks::<9>().0 {
        let v: Vec<[u32; 3]> = t
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()])
            .collect();
        for (a, b) in [(v[0], v[1]), (v[1], v[2]), (v[2], v[0])] {
            *edges
                .entry(if a < b { (a, b) } else { (b, a) })
                .or_insert(0) += 1;
        }
        let p = |i: usize| Vec3::new(t[i * 3] as f64, t[i * 3 + 1] as f64, t[i * 3 + 2] as f64);
        volume += p(0).dot(p(1).cross(p(2))) / 6.0;
    }
    (edges, volume)
}

fn point(x: f64, y: f64, z: f64, bulge: bool) -> ChainPoint {
    ChainPoint {
        pos: Vec3::new(x, y, z),
        bulge,
    }
}

#[test]
fn a_bent_chain_is_one_closed_outward_tube_with_a_bicone() {
    let params = SmoothShaftParams::default();
    let r = 0.5;
    let points = [
        point(0.0, 0.0, 0.0, false),
        point(0.0, 0.0, 10.0, true),
        point(4.0, 0.0, 16.0, false),
        point(4.0, 3.0, 22.0, true),
        point(4.0, 3.0, 30.0, false),
    ];
    let mut soup = TriangleSink::default();
    append_smooth_chain(&mut soup, &points, &[(r, r); 4], 12, &params);
    let soup = soup.floats;

    let (edges, volume) = topology(&soup);
    assert!(
        edges.values().all(|&c| c == 2),
        "closed: every edge shared by two triangles"
    );
    let length: f64 = points
        .windows(2)
        .map(|w| w[1].pos.distance_to(w[0].pos))
        .sum();
    let cylinder = std::f64::consts::PI * r * r * length;
    assert!(
        volume > 0.9 * cylinder,
        "outward-wound and as large as the shaft ({volume} vs {cylinder})"
    );
    assert!(
        volume < 1.6 * cylinder,
        "the bicones add, but not much ({volume} vs {cylinder})"
    );

    // The waist at a bulging joint, and nothing wider elsewhere.
    let waist = params.bulge_ratio * r;
    let widest_near = |c: Vec3, axis: Vec3| {
        soup.as_chunks::<3>()
            .0
            .iter()
            .map(|p| Vec3::new(p[0] as f64, p[1] as f64, p[2] as f64))
            .filter(|p| p.distance_to(c) < 2.0 * waist)
            .map(|p| {
                let d = p.sub(c);
                d.sub(axis.multiply_scalar(d.dot(axis))).length()
            })
            .fold(0.0_f64, f64::max)
    };
    let up = Vec3::new(0.0, 0.0, 1.0);
    assert!(
        (widest_near(points[1].pos, up) - waist).abs() < 0.2 * r,
        "bicone at the first joint"
    );
    assert!(
        widest_near(Vec3::new(0.0, 0.0, 3.0), up) <= r + 1e-4,
        "a plain shaft away from joints"
    );
}

#[test]
fn a_straight_chain_without_bulges_is_a_capped_cylinder() {
    let mut soup = TriangleSink::default();
    let points = [
        point(0.0, 0.0, 0.0, false),
        point(0.0, 0.0, 5.0, false),
        point(0.0, 0.0, 9.0, false),
    ];
    append_smooth_chain(
        &mut soup,
        &points,
        &[(1.0, 1.0), (1.0, 1.0)],
        16,
        &SmoothShaftParams::default(),
    );
    let soup = soup.floats;
    let (edges, volume) = topology(&soup);
    assert!(edges.values().all(|&c| c == 2));
    // A 16-gon prism of circumradius 1 and height 9.
    let prism = 0.5 * 16.0 * (std::f64::consts::TAU / 16.0).sin() * 9.0;
    assert!((volume - prism).abs() < 1e-3 * prism, "{volume} vs {prism}");
}

fn load(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn on_dragonfruit_states_smooth_shafts_stay_closed_and_drop_the_spheres() {
    let fixture = load("df_islands_c.json");
    let native = load("df_native_islands.json");
    let raft = RaftSettings {
        bottom_mode: RaftBottomMode::Off,
        ..DEFAULT_RAFT_SETTINGS
    };
    let mut checked = 0;
    for run in fixture["runs"].as_array().unwrap() {
        let Some(support) = run["result"].get("support") else {
            continue;
        };
        let scene = native["runs"][run["run"].as_u64().unwrap() as usize]["scene"]
            .as_str()
            .unwrap()
            .to_owned();
        let state: SupportCollections = serde_json::from_value(support.clone()).unwrap();
        let visible: HashSet<String> = [scene].into();
        let soup_of = |shaft_style| {
            build_support_and_raft_world_triangles(&SupportSliceInput {
                state: &state,
                visible_model_ids: &visible,
                raft: &raft,
                plate_clearance_models: &[],
                support_tip_shrink_percent: 0.0,
                tip_penetration_mm: 0.0,
                tip_drawing: Default::default(),
                shaft_style,
                robust_raft_geometry: false,
            })
            .unwrap()
        };
        let df = soup_of(ShaftStyle::DragonFruit);
        let smooth = soup_of(ShaftStyle::Smooth(SmoothShaftParams::default()));
        assert!(!smooth.is_empty());
        assert!(
            smooth.len() < df.len(),
            "no joint spheres: fewer triangles than DragonFruit's"
        );
        let open = |soup: &[f32]| topology(soup).0.values().filter(|&&c| c % 2 != 0).count();
        // Coincident DragonFruit primitives (contacts, roots) can leave odd
        // edges in the welded soup; the tubes never add any.
        assert!(
            open(&smooth) <= open(&df),
            "no more open edges than DragonFruit's soup"
        );
        assert!(smooth.iter().all(|v| v.is_finite()));
        checked += 1;
    }
    assert!(checked > 40, "{checked} states");
}

#[test]
fn a_joint_the_shaft_runs_straight_through_does_not_bulge() {
    let mut soup = TriangleSink::default();
    // Flagged as a joint, but collinear: no bicone.
    let points = [
        point(0.0, 0.0, 0.0, false),
        point(0.0, 0.0, 5.0, true),
        point(0.0, 0.0, 9.0, false),
    ];
    append_smooth_chain(
        &mut soup,
        &points,
        &[(1.0, 1.0); 2],
        16,
        &SmoothShaftParams::default(),
    );
    let soup = soup.floats;
    let widest = soup
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| (p[0] as f64).hypot(p[1] as f64))
        .fold(0.0_f64, f64::max);
    assert!(
        widest <= 1.0 + 1e-6,
        "no bulge on a straight joint ({widest})"
    );
}

#[test]
fn a_shaft_bends_into_its_contact_cone_without_a_gap() {
    // Up the shaft, then a 40-degree bend into a cone tapering to its tip.
    let points = [
        point(0.0, 0.0, 0.0, false),
        point(0.0, 0.0, 10.0, true),
        point(2.0, 0.0, 12.4, false),
    ];
    let (shaft, body, tip) = (0.5, 0.45, 0.15);
    let mut soup = TriangleSink::default();
    append_smooth_chain(
        &mut soup,
        &points,
        &[(shaft, shaft), (body, tip)],
        12,
        &SmoothShaftParams::default(),
    );
    let soup = soup.floats;
    let (edges, volume) = topology(&soup);
    assert!(
        edges.values().all(|&c| c == 2),
        "one closed piece through the bend"
    );
    assert!(volume > 0.0);
    // The far end is the tip's radius.
    let end = points[2].pos;
    let at_tip = soup
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| Vec3::new(p[0] as f64, p[1] as f64, p[2] as f64))
        .filter(|p| p.distance_to(end) < 0.3)
        .map(|p| p.distance_to(end))
        .fold(0.0_f64, f64::max);
    assert!((at_tip - tip).abs() < 1e-3, "tip radius {at_tip}");
}

/// Every vertex within `reach` of the chain's centre line.
fn stays_near(soup: &[f32], points: &[ChainPoint], reach: f64) -> bool {
    soup.as_chunks::<3>().0.iter().all(|p| {
        let q = Vec3::new(p[0] as f64, p[1] as f64, p[2] as f64);
        points.windows(2).any(|w| {
            let (a, b) = (w[0].pos, w[1].pos);
            let ab = b.sub(a);
            let t = (q.sub(a).dot(ab) / ab.dot(ab).max(1e-12)).clamp(0.0, 1.0);
            q.distance_to(a.add(ab.multiply_scalar(t))) <= reach
        })
    })
}

#[test]
fn hairpins_and_sharp_bends_stay_on_their_path() {
    let params = SmoothShaftParams::default();
    let r = 0.5;
    let waist = params.bulge_ratio * r;
    let cases: Vec<Vec<ChainPoint>> = vec![
        // A U-turn: up, then straight back down beside itself.
        vec![
            point(0.0, 0.0, 0.0, false),
            point(0.0, 0.0, 10.0, true),
            point(0.01, 0.0, 0.0, false),
        ],
        // Almost a U-turn into a cone.
        vec![
            point(0.0, 0.0, 0.0, false),
            point(0.0, 0.0, 10.0, true),
            point(0.5, 0.0, 7.0, false),
        ],
        // A zig-zag of 150-degree bends.
        vec![
            point(0.0, 0.0, 0.0, false),
            point(0.0, 0.0, 6.0, true),
            point(1.5, 0.0, 0.5, true),
            point(3.0, 0.0, 6.0, true),
            point(4.5, 0.0, 0.5, false),
        ],
    ];
    for (k, points) in cases.iter().enumerate() {
        let mut soup = TriangleSink::default();
        let radii = vec![(r, r); points.len() - 1];
        append_smooth_chain(&mut soup, points, &radii, 12, &params);
        let soup = soup.floats;
        assert!(soup.iter().all(|v| v.is_finite()), "case {k}: finite");
        assert!(
            stays_near(&soup, points, 2.0 * waist + 1e-6),
            "case {k}: no vertex flies off the path"
        );
    }
}

#[test]
fn on_dragonfruit_states_no_vertex_flies_off() {
    // Every vertex of the smooth soup lies within the bounds of the
    // DragonFruit soup grown by a few millimetres.
    let fixture = load("df_islands_c.json");
    let native = load("df_native_islands.json");
    let autobrace = load("df_autobrace.json");
    let raft = RaftSettings {
        bottom_mode: RaftBottomMode::Off,
        ..DEFAULT_RAFT_SETTINGS
    };
    let states = fixture["runs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| {
            let scene = native["runs"][r["run"].as_u64().unwrap() as usize]["scene"]
                .as_str()
                .unwrap();
            r["result"]
                .get("support")
                .map(|s| (scene.to_owned(), s.clone()))
        })
        .chain(
            autobrace["runs"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|r| {
                    r["result"]
                        .get("support")
                        .map(|s| (r["scene"].as_str().unwrap().to_owned(), s.clone()))
                }),
        );
    let bounds = |soup: &[f32]| {
        soup.as_chunks::<3>().0.iter().fold(
            [
                f32::INFINITY,
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ],
            |b, p| {
                [
                    b[0].min(p[0]),
                    b[1].min(p[1]),
                    b[2].min(p[2]),
                    b[3].max(p[0]),
                    b[4].max(p[1]),
                    b[5].max(p[2]),
                ]
            },
        )
    };
    let mut checked = 0;
    for (scene, support) in states {
        let state: SupportCollections = serde_json::from_value(support).unwrap();
        let visible: HashSet<String> = [scene].into();
        let soup_of = |shaft_style| {
            build_support_and_raft_world_triangles(&SupportSliceInput {
                state: &state,
                visible_model_ids: &visible,
                raft: &raft,
                plate_clearance_models: &[],
                support_tip_shrink_percent: 0.0,
                tip_penetration_mm: 0.0,
                tip_drawing: Default::default(),
                shaft_style,
                robust_raft_geometry: false,
            })
            .unwrap()
        };
        let (df, smooth) = (
            soup_of(ShaftStyle::DragonFruit),
            soup_of(ShaftStyle::Smooth(SmoothShaftParams::default())),
        );
        if df.is_empty() {
            continue;
        }
        let (d, s) = (bounds(&df), bounds(&smooth));
        let margin = 3.0;
        for k in 0..3 {
            assert!(
                s[k] >= d[k] - margin && s[k + 3] <= d[k + 3] + margin,
                "axis {k}: smooth {s:?} vs dragonfruit {d:?}"
            );
        }
        checked += 1;
    }
    assert!(checked > 100, "{checked} states");
}
