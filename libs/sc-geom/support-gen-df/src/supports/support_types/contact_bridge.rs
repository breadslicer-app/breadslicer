//! The registered contact bridge builders (`registerContactBridgeBuilder` in
//! `twigRegistration.ts` and `stickRegistration.ts`) and the registry's
//! `buildContactBridge` dispatch.
//!
//! A twig or stick bridges two model contacts, so it can be built from the type
//! id alone once the registry has chosen it by contact span. Each enforces its
//! own verticality gate here rather than at each caller: a bridge canted past it
//! is not a support, whichever automatic path built it. A manual placement is
//! exempt -- the user aimed both contacts and can see the cant in the preview.

use serde::Serialize;

use super::stick::stick_builder::{StickBuildInput, build_stick};
use super::stick::stick_verticality::is_shaft_vertical_enough;
use super::twig::twig_builder::{TwigBuildInput, build_twig};
use super::twig::twig_verticality::is_twig_shaft_vertical_enough;
use crate::supports::model_mesh::ModelMesh;
use crate::supports::settings::types::SupportSettings;
use crate::supports::support_type_registry::SupportTypeId;
use crate::supports::types::{LimitationCode, Stick, Twig, Vec3};

#[derive(Clone, Copy)]
pub struct ContactBridgeRequest<'a> {
    pub model_id: &'a str,
    pub a_pos: Vec3,
    pub a_normal: Vec3,
    pub b_pos: Vec3,
    pub b_normal: Vec3,
    pub shaft_diameter_mm: Option<f64>,
    pub tip_contact_diameter_mm: Option<f64>,
    /// Checked for clearance when given, so a preview can show the failure.
    pub mesh: Option<ModelMesh<'a>>,
    /// True when a person aimed both contacts by hand. Auto callers leave it
    /// unset and keep the cant caps.
    pub manual: Option<bool>,
}

/// The entity a bridge builder produced.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum BridgeEntity {
    Twig(Twig),
    Stick(Stick),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContactBridgeResult {
    pub entity: BridgeEntity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LimitationCode>,
}

/// `buildContactBridge(typeId, request)`: `None` for a type with no bridge
/// builder, or a bridge its verticality gate refused.
pub fn build_contact_bridge(
    type_id: SupportTypeId,
    request: &ContactBridgeRequest<'_>,
    settings: &SupportSettings,
    new_id: &mut dyn FnMut() -> String,
) -> Option<ContactBridgeResult> {
    let manual = request.manual == Some(true);
    match type_id {
        SupportTypeId::Twig => {
            let built = build_twig(
                &TwigBuildInput {
                    model_id: request.model_id,
                    a_pos: request.a_pos,
                    a_normal: request.a_normal,
                    b_pos: request.b_pos,
                    b_normal: request.b_normal,
                    mesh: request.mesh,
                    tip_contact_diameter_mm: request.tip_contact_diameter_mm,
                },
                settings,
                new_id,
            );
            (manual || is_twig_shaft_vertical_enough(&built.twig.segments)).then_some(
                ContactBridgeResult {
                    entity: BridgeEntity::Twig(built.twig),
                    error: built.error,
                },
            )
        }
        SupportTypeId::Stick => {
            let built = build_stick(
                &StickBuildInput {
                    model_id: request.model_id,
                    shaft_diameter_mm: request.shaft_diameter_mm,
                    tip_contact_diameter_mm: request.tip_contact_diameter_mm,
                    a_pos: request.a_pos,
                    a_normal: request.a_normal,
                    b_pos: request.b_pos,
                    b_normal: request.b_normal,
                    mesh: request.mesh,
                },
                settings,
                new_id,
            );
            (manual || is_shaft_vertical_enough(&built.stick.segments)).then_some(
                ContactBridgeResult {
                    entity: BridgeEntity::Stick(built.stick),
                    error: built.error,
                },
            )
        }
        _ => None,
    }
}
