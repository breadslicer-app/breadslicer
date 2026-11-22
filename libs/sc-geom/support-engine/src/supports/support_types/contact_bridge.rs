//! Building a bridge between two model contacts once its type is chosen.
//!
//! Twigs and sticks both join two contacts on the model, and the registry
//! picks between them by the span. Each type's lean limit is applied here,
//! not at each caller, so no automatic path can keep a bridge canted past
//! it. A manual bridge is exempt: the user aimed both ends and sees the lean
//! in the preview.

use serde::Serialize;

use super::stick::stick_builder::{StickBuildInput, build_stick};
use super::stick::stick_verticality::is_shaft_vertical_enough;
use super::twig::twig_builder::{TwigBuildInput, build_twig};
use super::twig::twig_verticality::is_twig_shaft_vertical_enough;
use crate::supports::model_mesh::ModelMesh;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_kinds::SupportTypeId;
use crate::supports::types::{LimitationCode, Segment, Stick, Twig, Vec3};

#[derive(Clone, Copy, Debug)]
pub struct ContactBridgeRequest<'a> {
    pub model_id: &'a str,
    pub a_pos: Vec3,
    pub a_normal: Vec3,
    pub b_pos: Vec3,
    pub b_normal: Vec3,
    pub shaft_diameter_mm: Option<f64>,
    pub tip_contact_diameter_mm: Option<f64>,
    /// When given, the bridge is checked against it, so a preview can show
    /// a collision.
    pub mesh: Option<ModelMesh<'a>>,
    /// Set when a person aimed both contacts by hand; automatic callers
    /// leave it unset and keep the lean limits.
    pub manual: Option<bool>,
}

/// The support a bridge was built as.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum BridgeEntity {
    Twig(Twig),
    Stick(Stick),
}

impl BridgeEntity {
    /// The bridge's shaft.
    pub fn segments(&self) -> &[Segment] {
        match self {
            BridgeEntity::Twig(twig) => &twig.segments,
            BridgeEntity::Stick(stick) => &stick.segments,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContactBridgeResult {
    pub entity: BridgeEntity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
}

/// The bridge of type `type_id` between the request's two contacts. `None`
/// when that type does not bridge contacts, or when the bridge leans past
/// its type's limit.
pub fn build_contact_bridge(
    type_id: SupportTypeId,
    request: &ContactBridgeRequest<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> Option<ContactBridgeResult> {
    let r = request;
    let (entity, error, upright) = match type_id {
        SupportTypeId::Twig => {
            let built = build_twig(
                &TwigBuildInput {
                    model_id: r.model_id,
                    a_pos: r.a_pos,
                    a_normal: r.a_normal,
                    b_pos: r.b_pos,
                    b_normal: r.b_normal,
                    mesh: r.mesh,
                    tip_contact_diameter_mm: r.tip_contact_diameter_mm,
                },
                settings,
                new_id,
            );
            let upright = is_twig_shaft_vertical_enough(&built.twig.segments);
            (BridgeEntity::Twig(built.twig), built.error, upright)
        }
        SupportTypeId::Stick => {
            let built = build_stick(
                &StickBuildInput {
                    model_id: r.model_id,
                    shaft_diameter_mm: r.shaft_diameter_mm,
                    tip_contact_diameter_mm: r.tip_contact_diameter_mm,
                    a_pos: r.a_pos,
                    a_normal: r.a_normal,
                    b_pos: r.b_pos,
                    b_normal: r.b_normal,
                    mesh: r.mesh,
                },
                settings,
                new_id,
            );
            let upright = is_shaft_vertical_enough(&built.stick.segments);
            (BridgeEntity::Stick(built.stick), built.error, upright)
        }
        _ => return None,
    };
    (upright || r.manual == Some(true)).then_some(ContactBridgeResult { entity, error })
}
