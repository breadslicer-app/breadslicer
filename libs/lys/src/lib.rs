/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! Lychee Slicer's scene files (`.lys`, #698), read so a person can keep
//! working in Breadslicer on the scenes they saved there. Worked out from the
//! person's own files only.
//!
//! A `.lys` is a small container: four `u32` (little-endian), the second the
//! length of what follows it up to the data, the fourth the length of a JSON
//! index; the index names each entry with its offset and size from the start
//! of the data. The entries:
//!
//! - `<hash>.bin`: a model's mesh, `[version, 12, index count, float count,
//!   0]` as `u32`, the triangles' vertex indices, then the vertices as `f32`
//!   triples, centred on their box.
//! - `scene.bin`: the scene, MessagePack with every byte raised by a key that
//!   repeats along it (the encoding Lychee's printer exports use, see
//!   `sc_print_profiles::import`).
//! - `<hash>_hollowing.bin`, the islands found, and a preview picture.
//!
//! A model stands scaled, rotated (degrees, about X, then Y, then Z of the
//! world), the centre of its mesh's box at `position.x, position.y` of its
//! plate and its lowest point at `position.z`: the height Lychee lifts it by.

use std::collections::BTreeMap;

use sc_mesh_core::transform::{AffineTransform3D, Transform3D};
use sc_mesh_core::{IndexedMesh, IndexedTriangle, MeshMetadata, Vertex};
use sc_print_profiles::import::msgpack::{self, Value};

/// The key each byte of the scene is raised by.
const KEY: &[u8; 36] = b"a8ee1146-8d03-4b69-8a67-59009a3f9ee7";

/// The container versions this reader has seen. Others are read too while
/// their layout holds, and turned down when it does not.
const KNOWN_VERSIONS: &[&str] = &["3.1.0"];

/// A scene as the file holds it.
#[derive(Clone, Debug)]
pub struct LysScene {
    /// The container's version, as the file states it.
    pub version: String,
    /// The Lychee version that saved it, when the scene says.
    pub app_version: Option<String>,
    pub models: Vec<LysModel>,
    /// The plates, in their order. Never empty.
    pub plates: Vec<LysPlate>,
    /// The printer the scene was saved for, by its name (brand and model).
    pub printer: Option<String>,
    /// The resin the scene was saved with.
    pub resin: Option<LysResin>,
    /// How many supports the scene holds. Breadslicer leaves them out for now.
    pub supports: usize,
    /// How many models are hollowed. Breadslicer leaves the hollowing out for
    /// now.
    pub hollowed: usize,
}

/// A model of the scene, in place on its plate.
#[derive(Clone, Debug)]
pub struct LysModel {
    pub name: String,
    pub mesh: IndexedMesh,
    /// Where it stands on its plate: the plate's centre is the origin, its
    /// floor `z = 0`.
    pub transform: Transform3D,
    /// Its plate, an index into [`LysScene::plates`].
    pub plate: usize,
    pub hidden: bool,
}

/// A plate of the scene.
#[derive(Clone, Debug, PartialEq)]
pub struct LysPlate {
    /// Its name; empty when the scene gives it none.
    pub name: String,
}

/// A resin, as the scene names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LysResin {
    pub brand: String,
    pub name: String,
}

/// Why a file is not a scene this reader can open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LysError {
    /// Not a `.lys` container, or a broken one.
    NotAScene(String),
    /// A container of a version whose layout this reader does not know.
    Version(String),
    /// A model's mesh is missing or broken.
    Mesh { model: String, why: String },
}

impl std::fmt::Display for LysError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LysError::NotAScene(why) => write!(f, "not a Lychee scene file ({why})"),
            LysError::Version(v) => {
                write!(
                    f,
                    "a Lychee scene of version {v}, which this build cannot read"
                )
            }
            LysError::Mesh { model, why } => write!(f, "the mesh of {model} is unreadable ({why})"),
        }
    }
}

impl std::error::Error for LysError {}

fn broken(why: impl Into<String>) -> LysError {
    LysError::NotAScene(why.into())
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// The container: its version and its entries by name.
struct Container<'a> {
    version: String,
    entries: BTreeMap<String, &'a [u8]>,
}

fn number_of(value: &serde_json::Value) -> Option<usize> {
    match value {
        serde_json::Value::Number(n) => n.as_u64().map(|n| n as usize),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

fn container(bytes: &[u8]) -> Result<Container<'_>, LysError> {
    let (Some(head), Some(json_len)) = (u32_at(bytes, 4), u32_at(bytes, 12)) else {
        return Err(broken("too short"));
    };
    let json = bytes
        .get(16..16 + json_len as usize)
        .ok_or_else(|| broken("the index runs past the end"))?;
    let index: serde_json::Value =
        serde_json::from_slice(json).map_err(|e| broken(format!("its index: {e}")))?;
    let version = index
        .get("version")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    let files = index
        .get("mangoFiles")
        .and_then(|f| f.as_object())
        .ok_or_else(|| broken("its index lists no entries"))?;
    let data = 8 + head as usize;
    let mut entries = BTreeMap::new();
    for (name, entry) in files {
        let (Some(offset), Some(size)) = (
            entry.get("offset").and_then(number_of),
            entry.get("size").and_then(number_of),
        ) else {
            return Err(broken(format!("entry {name} has no offset or size")));
        };
        let start = data
            .checked_add(offset)
            .ok_or_else(|| broken("an entry past the end"))?;
        let slice = start
            .checked_add(size)
            .and_then(|end| bytes.get(start..end))
            .ok_or_else(|| broken(format!("entry {name} runs past the end")))?;
        entries.insert(name.clone(), slice);
    }
    Ok(Container { version, entries })
}

/// The scene's bytes as they were before Lychee raised them.
fn lowered(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .zip(KEY.iter().cycle())
        .map(|(b, k)| b.wrapping_sub(*k))
        .collect()
}

/// A mesh entry: its triangles and its vertices.
fn mesh_of(bytes: &[u8], name: &str) -> Result<IndexedMesh, String> {
    let field = |i: usize| u32_at(bytes, 4 * i).ok_or("too short");
    let indices = field(2)? as usize;
    let floats = field(3)? as usize;
    if indices % 3 != 0 || floats % 3 != 0 {
        return Err("counts that are not triangles".to_owned());
    }
    let vertex_bytes = floats.checked_mul(4).ok_or("too many vertices")?;
    let index_bytes = bytes
        .len()
        .checked_sub(20 + vertex_bytes)
        .ok_or("shorter than its counts")?;
    let width = match (indices, index_bytes) {
        (0, 0) => 4,
        (n, b) if b == n * 4 => 4,
        (n, b) if b == n * 2 => 2,
        _ => return Err("indices of no width it knows".to_owned()),
    };
    let vertex_count = floats / 3;
    let at = |i: usize| -> u32 {
        let o = 20 + i * width;
        if width == 4 {
            u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap_or_default())
        } else {
            u32::from(u16::from_le_bytes(
                bytes[o..o + 2].try_into().unwrap_or_default(),
            ))
        }
    };
    let mut faces = Vec::with_capacity(indices / 3);
    for t in 0..indices / 3 {
        let tri = [at(3 * t), at(3 * t + 1), at(3 * t + 2)];
        if tri.iter().any(|&i| i as usize >= vertex_count) {
            return Err("a triangle names a vertex it does not have".to_owned());
        }
        faces.push(IndexedTriangle::from_raw_indices(tri));
    }
    let start = 20 + index_bytes;
    let vertices = bytes[start..]
        .chunks_exact(12)
        .map(|c| {
            let f = |k: usize| f32::from_le_bytes(c[k..k + 4].try_into().unwrap_or_default());
            Vertex::new([f(0), f(4), f(8)])
        })
        .collect::<Vec<_>>();
    if vertices
        .iter()
        .any(|v| v.coords().iter().any(|c| !c.is_finite()))
    {
        return Err("a vertex that is not a number".to_owned());
    }
    let meta = MeshMetadata {
        name: Some(name.to_owned()),
        ..MeshMetadata::default()
    };
    Ok(IndexedMesh::new(Some(meta), vertices, faces))
}

fn num(value: &Value, key: &str) -> Option<f64> {
    value.get(key)?.as_f64()
}

fn xyz(value: &Value, key: &str, or: f64) -> [f64; 3] {
    let v = value.get(key);
    let at = |k: &str| {
        v.and_then(|v| num(v, k))
            .filter(|n| n.is_finite())
            .unwrap_or(or)
    };
    [at("x"), at("y"), at("z")]
}

fn text(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// The entries of a `{byId, allIds}` table, in `allIds`' order when it has one.
fn table<'a>(value: Option<&'a Value>) -> Vec<(&'a str, &'a Value)> {
    let Some(by_id) = value.and_then(|v| v.get("byId")) else {
        return Vec::new();
    };
    let Value::Map(map) = by_id else {
        return Vec::new();
    };
    let order = value.and_then(|v| v.get("allIds"));
    match order {
        Some(Value::Array(ids)) => ids
            .iter()
            .filter_map(|id| {
                let id = id.as_str()?;
                map.get_key_value(id).map(|(k, v)| (k.as_str(), v))
            })
            .collect(),
        _ => map.iter().map(|(k, v)| (k.as_str(), v)).collect(),
    }
}

/// The selected entry of a `{selectedId, byId}` table.
fn selected(value: Option<&Value>) -> Option<&Value> {
    let value = value?;
    let id = value.get("selectedId")?.as_str()?;
    value.get("byId")?.get(id)
}

/// The rotation Lychee gives in degrees: about X, then Y, then Z of the world.
fn rotation(deg: [f64; 3]) -> [[f64; 3]; 3] {
    let (sx, cx) = deg[0].to_radians().sin_cos();
    let (sy, cy) = deg[1].to_radians().sin_cos();
    let (sz, cz) = deg[2].to_radians().sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    mul(&rz, &mul(&ry, &rx))
}

fn mul(a: &[[f64; 3]; 3], b: &[[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

/// Where a model of `mesh` stands: scaled and rotated, its mesh's origin
/// (the centre of its box) at `position`'s x and y, its lowest point at
/// `position`'s z.
fn placement(
    mesh: &IndexedMesh,
    position: [f64; 3],
    rot_deg: [f64; 3],
    scale: [f64; 3],
) -> Transform3D {
    let r = rotation(rot_deg);
    let mut linear = [[0.0; 3]; 3];
    for (i, row) in linear.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = r[i][j] * scale[j];
        }
    }
    let lowest = mesh
        .vertices()
        .iter()
        .map(|v| {
            let [x, y, z] = v.coords().map(f64::from);
            linear[2][0] * x + linear[2][1] * y + linear[2][2] * z
        })
        .fold(f64::INFINITY, f64::min);
    let lift = if lowest.is_finite() {
        position[2] - lowest
    } else {
        position[2]
    };
    Transform3D::Affine(AffineTransform3D::from_linear_and_translation(
        linear,
        [position[0], position[1], lift],
    ))
}

/// Reads the scene in `bytes`, a whole `.lys` file.
pub fn read_lys(bytes: &[u8]) -> Result<LysScene, LysError> {
    let file = container(bytes)?;
    let scene_bytes = file
        .entries
        .get("scene.bin")
        .ok_or_else(|| broken("it holds no scene"))?;
    let scene = msgpack::decode(&lowered(scene_bytes)).map_err(|e| {
        if KNOWN_VERSIONS.contains(&file.version.as_str()) {
            broken(format!("its scene: {}", e.0))
        } else {
            LysError::Version(file.version.clone())
        }
    })?;
    let objects = scene.get("objects").and_then(|o| o.get("present"));
    if objects.is_none() {
        return Err(if KNOWN_VERSIONS.contains(&file.version.as_str()) {
            broken("its scene lists no models")
        } else {
            LysError::Version(file.version.clone())
        });
    }

    // Plates: the scene's own, in their order; one when it names none.
    let plate_rows = table(scene.get("plates").and_then(|p| p.get("present")));
    let mut plates = Vec::new();
    let mut plate_index = BTreeMap::new();
    let mut plate_origin = Vec::new();
    for (id, plate) in &plate_rows {
        plate_index.insert(id.to_string(), plates.len());
        plate_origin.push(xyz(plate, "position", 0.0));
        plates.push(LysPlate {
            name: text(plate, "name").unwrap_or_default(),
        });
    }
    if plates.is_empty() {
        plates.push(LysPlate {
            name: String::new(),
        });
        plate_origin.push([0.0; 3]);
    }

    let mut models = Vec::new();
    let mut hollowed = 0;
    for (id, object) in table(objects) {
        if object.get("isOnScene").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let name = text(object, "name").unwrap_or_else(|| id.to_owned());
        let hash = object
            .get("properties")
            .and_then(|p| text(p, "hash"))
            .ok_or_else(|| LysError::Mesh {
                model: name.clone(),
                why: "the scene names no mesh".to_owned(),
            })?;
        let entry = file
            .entries
            .get(&format!("{hash}.bin"))
            .ok_or_else(|| LysError::Mesh {
                model: name.clone(),
                why: "its mesh is not in the file".to_owned(),
            })?;
        let mesh = mesh_of(entry, &name).map_err(|why| LysError::Mesh {
            model: name.clone(),
            why,
        })?;
        let plate = object
            .get("plateId")
            .and_then(Value::as_str)
            .and_then(|p| plate_index.get(p).copied())
            .unwrap_or(0);
        let origin = plate_origin[plate];
        let position = xyz(object, "position", 0.0);
        let on_plate = [
            position[0] - origin[0],
            position[1] - origin[1],
            position[2],
        ];
        let transform = placement(
            &mesh,
            on_plate,
            xyz(object, "rotation", 0.0),
            xyz(object, "scale", 1.0),
        );
        if object
            .get("hollowing")
            .and_then(|h| h.get("enabled"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            hollowed += 1;
        }
        models.push(LysModel {
            name,
            mesh,
            transform,
            plate,
            hidden: object.get("visible").and_then(Value::as_bool) == Some(false),
        });
    }

    let supports = match scene
        .get("supports")
        .and_then(|s| s.get("present"))
        .and_then(|p| p.get("byId"))
    {
        Some(Value::Map(map)) => map.len(),
        _ => 0,
    };
    let printer = selected(scene.get("printers")).and_then(|p| text(p, "name"));
    let resin = selected(scene.get("resins")).and_then(|r| {
        let name = text(r, "name")?;
        Some(LysResin {
            brand: text(r, "brand").unwrap_or_default(),
            name,
        })
    });
    Ok(LysScene {
        version: file.version,
        app_version: text(&scene, "appVersion"),
        models,
        plates,
        printer,
        resin,
        supports,
        hollowed,
    })
}

#[cfg(any(test, feature = "fixture"))]
pub mod fixture;
#[cfg(test)]
mod tests;
