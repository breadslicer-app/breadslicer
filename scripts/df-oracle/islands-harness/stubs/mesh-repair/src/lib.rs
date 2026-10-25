//! What DragonFruit's island commands import from `dragonfruit-mesh-repair`:
//! the re-exported mesh core. The file loader is not used by the harness.
pub use dragonfruit_mesh_core as core;
pub use dragonfruit_mesh_core::mesh::{IndexedMesh, Vec3};
pub mod io {
    pub fn load_mesh_from_path(_path: &std::path::Path) -> Result<super::IndexedMesh, String> {
        Err("the harness hands meshes in directly".into())
    }
}
