//! DragonFruit `src/supports/SupportTypes/Brace/`.
//!
//! A brace has no builder of its own: the manual flow writes the entity from
//! the placement controller, and auto-bracing from its planner. What is shared
//! geometry is ported here; the settle pass (`braceSettle.ts`) goes with the
//! store's knot recomputation, and placement state, preview batch, renderer,
//! proxy geometry and marquee shape are the GUI's.

pub mod brace_span_diameters;
