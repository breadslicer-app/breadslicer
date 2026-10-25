//! DragonFruit's native (Rust) island scanners, copied from its desktop build:
//! the voxel slice-growth scan (`rust/dragonfruit-islands`), the mesh-minima
//! and combined scan (`src-tauri/src/mesh_minima.rs`), the overhang and pose
//! stability classifier (`src-tauri/src/overhang.rs`) and the mesh core they
//! weld and query with (`rust/dragonfruit-mesh-core`). The desktop app runs
//! these, not the TypeScript fallback.
//!
//! The files stay as DragonFruit wrote them (bar the removed IPC shells and
//! the two marked order fixes), so they keep diffing cleanly against their
//! source: DragonFruit's own lint choices apply here, not this crate's.

#![allow(clippy::all, unused_imports)]

pub mod islands;
pub mod mesh_core;
pub mod mesh_minima;
pub mod overhang;
