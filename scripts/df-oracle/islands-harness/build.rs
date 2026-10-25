//! Copies DragonFruit's two command files into OUT_DIR without their leading
//! `//!` doc lines (inner attributes cannot be `include!`d). One more change:
//! `build_perimeter_loops` walks its edge map in iteration order, which a std
//! HashMap randomises per instance; its local containers become ordered ones,
//! the same algorithm in key order (the order SoulCrafted fixes).
fn main() {
    let out = std::env::var("OUT_DIR").unwrap();
    for name in ["overhang.rs", "mesh_minima.rs"] {
        let src = format!("C:/Users/dnaon/.openclaw/workspace/repos/DragonFruit/src-tauri/src/{name}");
        println!("cargo:rerun-if-changed={src}");
        let text = std::fs::read_to_string(&src).unwrap();
        let mut body: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("//!"))
            .map(|l| format!("{l}\n"))
            .collect();
        if name == "overhang.rs" {
            let local = "    use std::collections::{HashMap, HashSet};\n";
            assert_eq!(body.matches(local).count(), 1);
            body = body.replace(
                local,
                "    use std::collections::{BTreeMap as HashMap, BTreeSet as HashSet};\n",
            );
        }
        std::fs::write(format!("{out}/{name}"), body).unwrap();
    }
}
