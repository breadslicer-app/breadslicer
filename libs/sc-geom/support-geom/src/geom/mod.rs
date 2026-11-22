//! Vectors, matrices, rays, triangles and flat-shape extrusion for the support
//! engine, on `glam` and `earcutr`.

pub mod box3;
pub mod buffer_geometry;
pub mod extrude_geometry;
pub mod line3;
pub mod math_utils;
pub mod matrix3;
pub mod matrix4;
pub mod primitive_geometry;
pub mod quaternion;
pub mod ray;
pub mod shape;
pub mod shape_utils;
pub mod triangle;
pub mod vector2;
pub mod vector3;

pub use box3::Box3;
pub use line3::Line3;
pub use matrix3::Matrix3;
pub use matrix4::Matrix4;
pub use quaternion::Quaternion;
pub use ray::{Ray, RaySegmentDistance};
pub use triangle::Triangle;
pub use vector2::Vector2;
pub use vector3::{Vec3, Vector3};
