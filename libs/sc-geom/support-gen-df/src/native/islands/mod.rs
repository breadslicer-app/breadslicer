//! DragonFruit `rust/dragonfruit-islands`: the slice-growth voxel island
//! scanner (rasterize each layer to RLE, subtract the dilated previous layer,
//! track islands across layers). The streaming variant is not used by the
//! combined scan and is not copied.

pub mod geometry;
pub mod model;
pub mod pipeline;
pub mod rasterize;
pub mod rle;
pub mod scan;
pub mod tracker;
