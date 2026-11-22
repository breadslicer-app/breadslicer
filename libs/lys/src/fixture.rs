/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: Apache-2.0
 */

//! A small `.lys` written from scratch, for the tests of the crates that
//! open one (#698).

use crate::KEY;

/// A MessagePack value to write, the few types a scene uses.
#[allow(dead_code)]
#[derive(Debug)]
pub enum Mp {
    Nil,
    Bool(bool),
    Num(f64),
    Str(&'static str),
    Arr(Vec<Mp>),
    Map(Vec<(&'static str, Mp)>),
}

pub fn write(value: &Mp, out: &mut Vec<u8>) {
    match value {
        Mp::Nil => out.push(0xc0),
        Mp::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        Mp::Num(n) => {
            out.push(0xcb);
            out.extend_from_slice(&n.to_be_bytes());
        }
        Mp::Str(s) => {
            out.push(0xd9);
            out.push(s.len() as u8);
            out.extend_from_slice(s.as_bytes());
        }
        Mp::Arr(items) => {
            out.push(0xdc);
            out.extend_from_slice(&(items.len() as u16).to_be_bytes());
            for item in items {
                write(item, out);
            }
        }
        Mp::Map(pairs) => {
            out.push(0xde);
            out.extend_from_slice(&(pairs.len() as u16).to_be_bytes());
            for (k, v) in pairs {
                write(&Mp::Str(k), out);
                write(v, out);
            }
        }
    }
}

pub fn raised(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .zip(KEY.iter().cycle())
        .map(|(b, k)| b.wrapping_add(*k))
        .collect()
}

pub fn v3(x: f64, y: f64, z: f64) -> Mp {
    Mp::Map(vec![
        ("x", Mp::Num(x)),
        ("y", Mp::Num(y)),
        ("z", Mp::Num(z)),
    ])
}

/// A box 2 × 4 × 6 mm centred on the origin, as a mesh entry, its indices
/// `width` bytes wide.
pub fn box_entry(width: usize) -> Vec<u8> {
    let mut vertices = Vec::new();
    for k in 0..8u32 {
        let pick = |bit: u32, half: f32| if k & bit == 0 { -half } else { half };
        vertices.push([pick(1, 1.0), pick(2, 2.0), pick(4, 3.0)]);
    }
    let faces: [[u32; 3]; 12] = [
        [0, 2, 1],
        [1, 2, 3],
        [4, 5, 6],
        [5, 7, 6],
        [0, 1, 4],
        [1, 5, 4],
        [2, 6, 3],
        [3, 6, 7],
        [0, 4, 2],
        [2, 4, 6],
        [1, 3, 5],
        [3, 7, 5],
    ];
    let mut out = Vec::new();
    for word in [2u32, 12, 36, 24, 0] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    for i in faces.iter().flatten() {
        if width == 4 {
            out.extend_from_slice(&i.to_le_bytes());
        } else {
            out.extend_from_slice(&(*i as u16).to_le_bytes());
        }
    }
    for v in vertices.iter().flatten() {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

pub fn scene(objects: Vec<(&'static str, Mp)>, plates: Mp) -> Mp {
    let ids = objects.iter().map(|(k, _)| Mp::Str(k)).collect();
    Mp::Map(vec![
        ("appVersion", Mp::Str("7.0.0")),
        (
            "objects",
            Mp::Map(vec![(
                "present",
                Mp::Map(vec![("byId", Mp::Map(objects)), ("allIds", Mp::Arr(ids))]),
            )]),
        ),
        ("plates", plates),
        (
            "supports",
            Mp::Map(vec![(
                "present",
                Mp::Map(vec![(
                    "byId",
                    Mp::Map(vec![("s1", Mp::Nil), ("s2", Mp::Nil), ("s3", Mp::Nil)]),
                )]),
            )]),
        ),
        (
            "printers",
            Mp::Map(vec![
                ("selectedId", Mp::Str("p2")),
                (
                    "byId",
                    Mp::Map(vec![
                        ("p1", Mp::Map(vec![("name", Mp::Str("Other"))])),
                        ("p2", Mp::Map(vec![("name", Mp::Str("Creality Halot One"))])),
                    ]),
                ),
            ]),
        ),
        (
            "resins",
            Mp::Map(vec![
                ("selectedId", Mp::Str("r1")),
                (
                    "byId",
                    Mp::Map(vec![(
                        "r1",
                        Mp::Map(vec![
                            ("name", Mp::Str("Standard Rigid Plus ")),
                            ("brand", Mp::Str("Creality")),
                        ]),
                    )]),
                ),
            ]),
        ),
    ])
}

pub fn object(
    name: &'static str,
    plate: &'static str,
    position: Mp,
    rotation: Mp,
    scale: Mp,
) -> Mp {
    Mp::Map(vec![
        ("name", Mp::Str(name)),
        ("plateId", Mp::Str(plate)),
        ("position", position),
        ("rotation", rotation),
        ("scale", scale),
        ("properties", Mp::Map(vec![("hash", Mp::Str("abc"))])),
        ("hollowing", Mp::Map(vec![("enabled", Mp::Bool(true))])),
    ])
}

pub fn two_plates() -> Mp {
    let plate = |name: &'static str, x: f64| {
        Mp::Map(vec![("name", Mp::Str(name)), ("position", v3(x, 0.0, 0.0))])
    };
    Mp::Map(vec![(
        "present",
        Mp::Map(vec![
            (
                "byId",
                Mp::Map(vec![
                    ("pl0", plate("", 0.0)),
                    ("pl1", plate("Second", 178.0)),
                ]),
            ),
            ("allIds", Mp::Arr(vec![Mp::Str("pl0"), Mp::Str("pl1")])),
        ]),
    )])
}

/// A whole `.lys` holding `scene` and the box mesh as `abc.bin`.
pub fn file(version: &str, scene: &Mp, mesh: &[u8]) -> Vec<u8> {
    let mut packed = Vec::new();
    write(scene, &mut packed);
    let scene = raised(&packed);
    let index = format!(
        r#"{{"version":"{version}","mangoFiles":{{"abc.bin":{{"size":{},"offset":"0"}},"scene.bin":{{"size":{},"offset":{}}}}}}}"#,
        mesh.len(),
        scene.len(),
        mesh.len()
    );
    let json = index.as_bytes();
    let mut out = Vec::new();
    let head = 8 + json.len() as u32;
    for word in [4u32, head, head - 4, json.len() as u32] {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(json);
    out.extend_from_slice(mesh);
    out.extend_from_slice(&scene);
    out
}

/// A scene of one box 2 × 4 × 6 mm on the second of two plates (the second
/// named "Second", 178 mm right of the first), 10 mm right of that plate's
/// centre and lifted 3 mm, for a Creality Halot One, with three supports
/// and its hollowing on.
pub fn box_on_second_plate() -> Vec<u8> {
    let scene = scene(
        vec![(
            "o1",
            object(
                "Box",
                "pl1",
                v3(188.0, 5.0, 3.0),
                v3(0.0, 0.0, 90.0),
                v3(2.0, 1.0, 1.0),
            ),
        )],
        two_plates(),
    );
    file("3.1.0", &scene, &box_entry(4))
}
