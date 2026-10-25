//! DragonFruit's own tests for the block 5 builders, ported:
//! `manualBridgeCant.test.ts`, `twigVerticality.test.ts`, `twigSizing.test.ts`
//! and `Kickstand/__tests__/kickstandColumnShape.test.ts`. (`stickSizing`
//! needs the auto-support sizing band and comes with the Auto;
//! `kickstandCollision` is covered by the parity fixture's mesh cases.)

use support_gen_df::supports::settings::types::SupportSettings;
use support_gen_df::supports::support_type_registry::SupportTypeId;
use support_gen_df::supports::support_types::contact_bridge::*;
use support_gen_df::supports::support_types::kickstand::kickstand_builder::*;
use support_gen_df::supports::support_types::kickstand::types::KickstandHostTarget;
use support_gen_df::supports::support_types::shaft_verticality::shaft_vertical_cos;
use support_gen_df::supports::support_types::stick::stick_verticality::MAX_SHAFT_ANGLE_DEG;
use support_gen_df::supports::support_types::twig::twig_builder::*;
use support_gen_df::supports::support_types::twig::twig_verticality::MAX_TWIG_SHAFT_ANGLE_DEG;
use support_gen_df::supports::types::{LimitationCode, Segment, Vec3};

fn ids() -> impl FnMut() -> String {
    let mut next = 0;
    move || {
        next += 1;
        format!("id-{next}")
    }
}

fn shaft_angle_deg(segments: &[Segment]) -> f64 {
    shaft_vertical_cos(segments).acos().to_degrees()
}

fn bridge(type_id: SupportTypeId, b_pos: Vec3, manual: bool) -> Option<ContactBridgeResult> {
    build_contact_bridge(
        type_id,
        &ContactBridgeRequest {
            model_id: "m",
            a_pos: Vec3::new(0.0, 0.0, 10.0),
            a_normal: Vec3::new(0.0, 0.0, -1.0),
            b_pos,
            b_normal: Vec3::new(0.0, 0.0, 1.0),
            shaft_diameter_mm: None,
            tip_contact_diameter_mm: None,
            mesh: None,
            manual: manual.then_some(true),
        },
        &SupportSettings::default(),
        &mut ids(),
    )
}

fn segments_of(result: &ContactBridgeResult) -> &[Segment] {
    match &result.entity {
        BridgeEntity::Twig(twig) => &twig.segments,
        BridgeEntity::Stick(stick) => &stick.segments,
    }
}

#[test]
fn manually_aimed_bridges_build_past_the_cant_the_auto_pass_refuses() {
    // Six mm across for a half mm of drop.
    let whisker = Vec3::new(6.0, 0.0, 9.5);
    for (type_id, gate) in [
        (SupportTypeId::Twig, MAX_TWIG_SHAFT_ANGLE_DEG),
        (SupportTypeId::Stick, MAX_SHAFT_ANGLE_DEG),
    ] {
        assert!(
            bridge(type_id, whisker, false).is_none(),
            "{type_id}: auto refuses"
        );
        let aimed = bridge(type_id, whisker, true).expect("a manual aim builds it");
        assert!(
            shaft_angle_deg(segments_of(&aimed)) > gate,
            "{type_id}: past the gate"
        );
    }
}

#[test]
fn a_twig_is_refused_when_too_canted_and_its_gate_is_the_twig_rule() {
    let vertical = bridge(SupportTypeId::Twig, Vec3::new(0.0, 0.0, 6.0), false).unwrap();
    assert!(shaft_angle_deg(segments_of(&vertical)) < 5.0);
    assert!(bridge(SupportTypeId::Twig, Vec3::new(6.0, 0.0, 9.5), false).is_none());

    // A cant a twig keeps but a stick would not.
    let prop = bridge(SupportTypeId::Twig, Vec3::new(1.0, 0.0, 7.5), false).unwrap();
    let angle = shaft_angle_deg(segments_of(&prop));
    assert!(angle > 20.0 && angle < MAX_TWIG_SHAFT_ANGLE_DEG, "{angle}");
}

#[test]
fn twigs_are_cylindrical() {
    let build = |z: f64| {
        build_twig(
            &TwigBuildInput {
                model_id: "m",
                a_pos: Vec3::new(0.0, 0.0, 10.0),
                a_normal: Vec3::new(0.0, 0.0, -1.0),
                b_pos: Vec3::new(0.0, 0.0, z),
                b_normal: Vec3::new(0.0, 0.0, 1.0),
                mesh: None,
                tip_contact_diameter_mm: None,
            },
            &SupportSettings::default(),
            &mut ids(),
        )
    };
    let ends = |z: f64| {
        let result = build(z);
        assert!(result.error.is_none());
        let seg = &result.twig.segments[0];
        (
            seg.bottom_joint.as_ref().unwrap().diameter,
            seg.top_joint.as_ref().unwrap().diameter,
        )
    };
    let (short_bottom, short_top) = ends(13.0);
    let (long_bottom, long_top) = ends(35.0);
    // Host end: 0.9× the tip contact (0.3 default) → joints under 0.33.
    assert!(short_bottom < 0.3 * 1.1, "{short_bottom}");
    assert!((short_top - short_bottom).abs() < 1e-9);
    assert!((long_top - long_bottom).abs() < 1e-9);
    assert!((long_top - short_top).abs() < 1e-9, "span-independent");
}

fn kickstand(
    root: Vec3,
    host_pos: Vec3,
    t: f64,
    diameter: f64,
    max: Option<f64>,
) -> Option<LimitationCode> {
    let host = KickstandHostTarget {
        segment_id: "shaft-1".to_owned(),
        support_kind: SupportTypeId::Trunk,
        t,
        pos: host_pos,
        diameter_mm: diameter,
        min_t: None,
    };
    build_kickstand_data(
        &KickstandBuildInput {
            model_id: "model-1",
            root_pos: root,
            host: &host,
            layout_overrides: None,
            authored_column_top: None,
            collision_mesh: None,
            max_shaft_angle_deg: max,
        },
        &SupportSettings::default(),
        &mut ids(),
    )
    .unwrap()
    .error
}

#[test]
fn kickstand_column_shape() {
    // The grid-snapped placement from DragonFruit's bug report: the chain
    // overshoots a low host and the terminal run doubles back down to it.
    assert_eq!(
        kickstand(
            Vec3::new(12.0, 16.0, 0.0),
            Vec3::new(9.096829382802428, 14.180691801174177, 2.8379237189740247),
            0.0096770214952281,
            1.3,
            None,
        ),
        Some(LimitationCode::AngleTooSteep)
    );
    // A column rising straight to a host above it is allowed.
    assert_eq!(
        kickstand(Vec3::ZERO, Vec3::new(0.0, 0.0, 20.0), 0.5, 2.0, None),
        None
    );
    // The limit comes from the shaft setting.
    let leaning = Vec3::new(8.0, 0.0, 20.0);
    assert_eq!(kickstand(Vec3::ZERO, leaning, 0.5, 2.0, None), None);
    assert_eq!(
        kickstand(Vec3::ZERO, leaning, 0.5, 2.0, Some(5.0)),
        Some(LimitationCode::AngleTooSteep)
    );
}
