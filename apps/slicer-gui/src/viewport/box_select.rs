/*
 * SPDX-FileCopyrightText: 2026 SynrgStudio
 * SPDX-License-Identifier: AGPL-3.0-only
 */

//! The objects a selection box touches on screen (#13).
//!
//! Shift+left drag draws a box on the viewport; on release, every object
//! whose picture on screen touches the box joins the selection. Touching,
//! not lying fully inside: a box drawn over part of a crowded plate takes
//! what it covers without having to reach around whole models, as in most
//! editors; Shift+click takes out what came along by mistake.
//!
//! An object touches the box when one of its triangles, as the camera
//! projects it, overlaps the box. Most objects are settled by their world
//! box alone: one whose box lands off the box on screen is out, and one whose
//! box lands wholly inside is in. Only those in between walk their vertices,
//! stopping at the first one inside the box, and then their triangles.

use glam::{Mat4, Vec3, Vec4};
use sc_mesh_core::transform::Transform3D;
use sc_mesh_spatial::SceneSpatial;
use sc_scene::NodeId;

use crate::camera::OrbitCamera;

/// A box on screen, in logical pixels, `min` the top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ScreenRect {
    min: [f32; 2],
    max: [f32; 2],
}

impl ScreenRect {
    /// The box with corners `a` and `b`, in any order.
    pub(super) fn spanning(a: [f32; 2], b: [f32; 2]) -> Self {
        Self {
            min: [a[0].min(b[0]), a[1].min(b[1])],
            max: [a[0].max(b[0]), a[1].max(b[1])],
        }
    }

    fn holds(&self, p: [f32; 2]) -> bool {
        (self.min[0]..=self.max[0]).contains(&p[0]) && (self.min[1]..=self.max[1]).contains(&p[1])
    }
}

/// Projects world points onto the viewport as the renderer draws them.
struct Projector {
    view_proj: Mat4,
    eye: Vec3,
    forward: Vec3,
    size: [f32; 2],
}

impl Projector {
    fn new(camera: &OrbitCamera, fov_y: f32, width: f32, height: f32) -> Self {
        let aspect = width / height.max(1.0);
        // As `picking::project_world_to_screen`: the depths only affect z.
        let view_proj = camera.projection(fov_y, aspect, 0.01, 100.0) * camera.view_matrix();
        Self {
            view_proj,
            eye: camera.eye(),
            forward: camera.target - camera.eye(),
            size: [width, height],
        }
    }

    /// The pixel `p` lands on; `None` behind the eye.
    fn project(&self, p: Vec3) -> Option<[f32; 2]> {
        let clip = self.view_proj * Vec4::new(p.x, p.y, p.z, 1.0);
        if clip.w <= 0.0 || (p - self.eye).dot(self.forward) <= 0.0 {
            return None;
        }
        let (x, y) = (clip.x / clip.w, clip.y / clip.w);
        let pixel = [
            (x + 1.0) * 0.5 * self.size[0],
            (1.0 - y) * 0.5 * self.size[1],
        ];
        pixel.iter().all(|c| c.is_finite()).then_some(pixel)
    }
}

/// The leaves, in row order, whose picture the box `rect` touches, each
/// passed through `selects` (the supports outside the Supports tab give
/// their model, #120; `None` drops a leaf), each once.
pub(super) fn leaves_touching(
    spatial: &SceneSpatial,
    camera: &OrbitCamera,
    fov_y: f32,
    viewport: [f32; 2],
    rect: ScreenRect,
    mut selects: impl FnMut(NodeId) -> Option<NodeId>,
) -> Vec<NodeId> {
    let projector = Projector::new(camera, fov_y, viewport[0], viewport[1]);
    let mut out: Vec<NodeId> = Vec::new();
    for id in (0..).map_while(|i| spatial.leaf_id_at_index(i)) {
        if !touches(spatial, id, &projector, rect) {
            continue;
        }
        if let Some(chosen) = selects(id)
            && !out.contains(&chosen)
        {
            out.push(chosen);
        }
    }
    out
}

/// Whether the picture of leaf `id` touches `rect`.
fn touches(spatial: &SceneSpatial, id: NodeId, projector: &Projector, rect: ScreenRect) -> bool {
    let Some(query) = spatial.leaf(id) else {
        return false;
    };
    let world = query.world_transform();
    let Some(aabb) = query.transformed_aabb(world) else {
        return false;
    };
    let (lo, hi) = (
        Vec3::from(aabb.min().coords()),
        Vec3::from(aabb.max().coords()),
    );
    let corners = (0..8).map(|k| {
        Vec3::new(
            if k & 1 == 0 { lo.x } else { hi.x },
            if k & 2 == 0 { lo.y } else { hi.y },
            if k & 4 == 0 { lo.z } else { hi.z },
        )
    });
    // The box's picture holds the object's. With a corner behind the eye it
    // says nothing, and the triangles decide.
    if let Some(shadow) = corners
        .map(|c| projector.project(c))
        .collect::<Option<Vec<_>>>()
    {
        let (mut min, mut max) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
        for p in shadow {
            for k in 0..2 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        if max[0] < rect.min[0]
            || min[0] > rect.max[0]
            || max[1] < rect.min[1]
            || min[1] > rect.max[1]
        {
            return false;
        }
        if rect.holds(min) && rect.holds(max) {
            return true;
        }
    }
    let model = match world {
        Transform3D::Identity => Mat4::IDENTITY,
        Transform3D::Affine(a) => {
            let (l, t) = (a.linear(), a.translation());
            Mat4::from_cols(
                Vec4::new(l[0][0] as f32, l[1][0] as f32, l[2][0] as f32, 0.0),
                Vec4::new(l[0][1] as f32, l[1][1] as f32, l[2][1] as f32, 0.0),
                Vec4::new(l[0][2] as f32, l[1][2] as f32, l[2][2] as f32, 0.0),
                Vec4::new(t[0] as f32, t[1] as f32, t[2] as f32, 1.0),
            )
        }
    };
    let mesh = query.mesh();
    let mut pixels: Vec<Option<[f32; 2]>> = Vec::with_capacity(mesh.vertices.len());
    for v in &mesh.vertices {
        let pixel = projector.project(model.transform_point3(Vec3::from(v.coords())));
        if pixel.is_some_and(|p| rect.holds(p)) {
            return true;
        }
        pixels.push(pixel);
    }
    mesh.faces.iter().any(|face| {
        let corner = |k: usize| pixels.get(usize::from(face.vertices[k])).copied().flatten();
        match (corner(0), corner(1), corner(2)) {
            (Some(a), Some(b), Some(c)) => triangle_touches(a, b, c, rect),
            _ => false,
        }
    })
}

/// Whether the triangle `a b c` on screen overlaps `rect`: no axis of
/// either separates them.
fn triangle_touches(a: [f32; 2], b: [f32; 2], c: [f32; 2], rect: ScreenRect) -> bool {
    let tri = [a, b, c];
    for k in 0..2 {
        let lo = a[k].min(b[k]).min(c[k]);
        let hi = a[k].max(b[k]).max(c[k]);
        if hi < rect.min[k] || lo > rect.max[k] {
            return false;
        }
    }
    let corners = [
        rect.min,
        [rect.max[0], rect.min[1]],
        rect.max,
        [rect.min[0], rect.max[1]],
    ];
    for i in 0..3 {
        let (p, q) = (tri[i], tri[(i + 1) % 3]);
        let n = [q[1] - p[1], p[0] - q[0]];
        let along = |v: [f32; 2]| n[0] * v[0] + n[1] * v[1];
        let (t_lo, t_hi) = tri
            .iter()
            .map(|&v| along(v))
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| {
                (lo.min(x), hi.max(x))
            });
        let (r_lo, r_hi) = corners
            .iter()
            .map(|&v| along(v))
            .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), x| {
                (lo.min(x), hi.max(x))
            });
        if t_hi < r_lo || r_hi < t_lo {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sc_mesh_core::transform::AffineTransform3D;
    use sc_mesh_fixtures::unit_cube_mesh;
    use sc_scene::{LeafPayload, Scene};

    use super::*;
    use crate::camera::AxisView;
    use crate::mesh_render::default_fov_y;

    const VIEW: [f32; 2] = [200.0, 100.0];

    /// Cubes scaled by `size` at each centre, each a root, with an
    /// orthographic camera looking straight down at the origin.
    fn plate(cubes: &[([f64; 3], f64)]) -> (SceneSpatial, OrbitCamera) {
        let mut scene = Scene::new();
        for &(at, size) in cubes {
            let leaf = scene
                .add_leaf(
                    None,
                    Transform3D::Affine(
                        AffineTransform3D::from_translation(at)
                            .compose(&AffineTransform3D::from_uniform_scale(size)),
                    ),
                    LeafPayload::Mesh(Arc::new(unit_cube_mesh())),
                )
                .expect("a leaf");
            scene.add_root(leaf).expect("a root");
        }
        let mut camera = OrbitCamera::new(Vec3::ZERO, 30.0);
        camera.set_axis_view(AxisView::Top);
        camera.orthographic = true;
        (SceneSpatial::from_graph(scene), camera)
    }

    fn pixel_of(camera: &OrbitCamera, p: [f32; 3]) -> [f32; 2] {
        let (x, y) = crate::picking::project_world_to_screen(
            camera,
            default_fov_y(),
            VIEW[0],
            VIEW[1],
            Vec3::from(p),
        )
        .expect("in view");
        [x, y]
    }

    fn touched(
        spatial: &SceneSpatial,
        camera: &OrbitCamera,
        a: [f32; 2],
        b: [f32; 2],
    ) -> Vec<usize> {
        leaves_touching(
            spatial,
            camera,
            default_fov_y(),
            VIEW,
            ScreenRect::spanning(a, b),
            Some,
        )
        .into_iter()
        .map(|id| spatial.leaf_index_of(id).expect("a live leaf"))
        .collect()
    }

    #[test]
    fn a_box_takes_what_it_touches_and_leaves_the_rest() {
        let (spatial, camera) = plate(&[([-4.0, 0.0, 0.0], 1.0), ([4.0, 0.0, 0.0], 1.0)]);
        // Around the left cube only.
        let a = pixel_of(&camera, [-6.0, 2.0, 0.0]);
        let b = pixel_of(&camera, [-2.0, -2.0, 0.0]);
        assert_eq!(touched(&spatial, &camera, a, b), vec![0]);
        // Over part of each: both, in row order, whichever corner it starts from.
        let a = pixel_of(&camera, [4.2, 0.2, 0.0]);
        let b = pixel_of(&camera, [-4.2, -0.2, 0.0]);
        assert_eq!(touched(&spatial, &camera, a, b), vec![0, 1]);
        // Between them: nothing.
        let a = pixel_of(&camera, [-2.0, 2.0, 0.0]);
        let b = pixel_of(&camera, [2.0, -2.0, 0.0]);
        assert!(touched(&spatial, &camera, a, b).is_empty());
    }

    #[test]
    fn a_box_inside_one_face_touches_its_object() {
        // A big cube: the box holds none of its vertices, nor its box's
        // corners, and still lies on it.
        let (spatial, camera) = plate(&[([0.0, 0.0, 0.0], 20.0)]);
        let a = pixel_of(&camera, [-1.0, 1.0, 0.0]);
        let b = pixel_of(&camera, [1.0, -1.0, 0.0]);
        assert_eq!(touched(&spatial, &camera, a, b), vec![0]);
    }

    #[test]
    fn the_supports_give_what_selects_them() {
        let (spatial, camera) = plate(&[([-4.0, 0.0, 0.0], 1.0), ([4.0, 0.0, 0.0], 1.0)]);
        let first = spatial.leaf_id_at_index(0).expect("a leaf");
        let second = spatial.leaf_id_at_index(1).expect("a leaf");
        // The second stands in for supports of the first: it gives the first,
        // once.
        let a = pixel_of(&camera, [-6.0, 2.0, 0.0]);
        let b = pixel_of(&camera, [6.0, -2.0, 0.0]);
        let got = leaves_touching(
            &spatial,
            &camera,
            default_fov_y(),
            VIEW,
            ScreenRect::spanning(a, b),
            |id| Some(if id == second { first } else { id }),
        );
        assert_eq!(got, vec![first]);
    }

    #[test]
    fn a_triangle_crossing_the_box_without_a_corner_in_it_touches() {
        let rect = ScreenRect::spanning([0.0, 0.0], [10.0, 10.0]);
        // A long thin triangle through the middle of the box.
        assert!(triangle_touches(
            [-50.0, 4.0],
            [60.0, 5.0],
            [-50.0, 6.0],
            rect
        ));
        // Off a corner of the box, with boxes that overlap: only the
        // triangle's long edge separates them.
        assert!(!triangle_touches(
            [5.0, -10.0],
            [25.0, -10.0],
            [25.0, 10.0],
            rect
        ));
        // Off to one side.
        assert!(!triangle_touches(
            [12.0, 0.0],
            [30.0, 0.0],
            [30.0, -18.0],
            rect
        ));
    }
}
