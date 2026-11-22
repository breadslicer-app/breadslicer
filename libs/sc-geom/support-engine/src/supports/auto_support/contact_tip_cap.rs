//! DragonFruit `src/supports/autoSupport/contactTipCap.ts`.
//!
//! Tip contact cap by local free width. A contact tip is a disc sitting on the
//! model and it must FIT the feature it lands on. The free width is the width
//! of the free span at the contact, measured in the plane the tip lands on:
//! the probe steps out along the tangent plane `CONTACT_WIDTH_PROBE_MM` in each
//! of the four tangent directions and reads how far it walked past the
//! feature's silhouette (`reach - d`); the sum over the two signs of the
//! narrower axis is the span the disc has to fit inside. The SDF is the oracle
//! the rest of placement routes against, so this queries the same field.

