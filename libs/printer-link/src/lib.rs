/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Sends a printer file to a resin printer on the local network, and starts
//! the print. One protocol so far: SDCP v3, the one Elegoo's network printers
//! speak ([`sdcp`]); and shows a printer's camera ([`camera`]). Everything
//! here blocks: callers run it on a worker.
//!
//! The protocol as it is used here follows Elegoo's published SDCP v3
//! documentation and the Open Resin Alliance's SDCP plugin for DragonFruit
//! (MIT), which sends to the same printers.

pub mod camera;
pub mod http;
pub mod sdcp;

#[cfg(test)]
mod tests_camera;
#[cfg(test)]
mod tests_sdcp;
