//! DragonFruit `supports/rendering/calculateTipOffset.ts` for the default
//! material anti-aliasing settings (`DEFAULT_MATERIAL_ANTI_ALIASING_SETTINGS`,
//! no custom override): how far the slice export sinks contact tips into the
//! model, so the anti-aliasing blur does not leave them short of it.
//!
//! Without an override, `resolveEffectiveAaSettings` takes the physical
//! auto configuration of `autoAaPhysics.ts` for the `balanced` preset, of
//! which the offset reads only the Z blur radius: `(2 r + 1)` layers. The
//! custom-override branches read DragonFruit material fields SoulCrafted's
//! profiles do not have, and are not ported.

