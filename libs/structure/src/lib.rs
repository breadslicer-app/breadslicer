/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A 3D beam frame for the supports of a resin print (#242).
//!
//! While a layer peels, the film pulls the model printed so far away from
//! the plate. The supports hold it: every trunk, branch, brace and tip is a
//! beam, the model is a rigid body, the raft holds the roots. This crate
//! finds what every beam carries, and how close it comes to breaking or
//! buckling.
//!
//! # The model
//!
//! - A [`Frame`] is nodes and round beams between them, tapered from one
//!   end to the other. Nodes closer than [`MERGE_MM`] are one node: a span
//!   that short would ruin the matrix.
//! - A beam is a Timoshenko beam with six degrees of freedom per node: its
//!   shear matters on the short tip necks, and leaves a long one an
//!   Euler–Bernoulli beam.
//! - A node is free, fixed (in the raft), or part of a rigid [`Body`] (a
//!   tip's end in the model): it moves with the body, `u = u_c + θ × r`,
//!   a kinematic constraint and not a stiff spring.
//! - [`solve`] factorises the stiffness once ([`faer`]'s sparse Cholesky,
//!   after Jacobi scaling, with one step of iterative refinement) and solves
//!   every load case with it. Whatever no fixed node or body holds is left
//!   out: it would make the matrix singular, and it carries nothing.
//!
//! # Stresses do not depend on E
//!
//! The frame is built of one material, so its forces, and the stresses they
//! cause, are the same whatever its stiffness: E only sets how far it moves,
//! and when a beam buckles.
//!
//! # Units
//!
//! Lengths in millimetres, forces in newtons, stresses and moduli in MPa
//! (newtons per square millimetre).
//!
//! # Not calibrated
//!
//! The material's strength and stiffness in the vat, and the sideways share
//! of the peel, are estimates. Until test prints calibrate them, the
//! utilisations compare supports with one another and say nothing absolute
//! about whether one holds.

mod check;
mod element;
mod frame;
mod solve;
mod supports;

pub use check::{MemberCheck, Sections, buckling_runs, member_check};
pub use frame::{Body, Frame, Hold, MERGE_MM, Member, Point};
pub use solve::{Load, Loads, Material, MemberForces, SolveError, Solved, Target, solve};
pub use supports::{
    BodyLoad, CheckOptions, GraphContact, GraphFoot, LayerCheck, NECK_KT, SupportGraph, check_layer,
};

#[cfg(test)]
mod tests_frame;
#[cfg(test)]
mod tests_supports;
