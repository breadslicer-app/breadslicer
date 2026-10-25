//! three-mesh-bvh 0.8.3 `src/core/build/{buildTree,computeBoundsUtils,splitUtils,sortUtils.generated}.js`.
//!
//! Only the `CENTER` strategy is ported: it is the library default and the only
//! one DragonFruit builds with (`accelerateGeometry` passes no options).

use super::{Node, NodeKind};

/// The build options DragonFruit uses (three-mesh-bvh `DEFAULT_OPTIONS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MeshBvhOptions {
    pub max_depth: u32,
    pub max_leaf_tris: usize,
}

impl Default for MeshBvhOptions {
    fn default() -> Self {
        MeshBvhOptions {
            max_depth: 40,
            max_leaf_tris: 10,
        }
    }
}

/// EPSILON for computing floating point error during build
const FLOAT32_EPSILON: f64 = 5.960_464_477_539_063e-8; // Math.pow( 2, - 24 )

/// The tree being built, before packing.
enum BuildNode {
    Leaf {
        bounds: [f32; 6],
        offset: usize,
        count: usize,
    },
    Inner {
        bounds: [f32; 6],
        split_axis: u32,
        left: Box<BuildNode>,
        right: Box<BuildNode>,
    },
}

/// `buildPackedTree`: one root per group range; a geometry with no groups and
/// the default draw range has one, over every triangle.
pub(super) fn build_packed_tree(
    positions: &[[f32; 3]],
    index: &mut [u32],
    options: MeshBvhOptions,
) -> Vec<Vec<Node>> {
    let tri_count = index.len() / 3;
    let mut triangle_bounds = compute_triangle_bounds(positions, index);
    if tri_count == 0 {
        return Vec::new();
    }
    let root = build_tree(&mut triangle_bounds, index, 0, tri_count, options);
    let mut packed = Vec::new();
    populate_buffer(&root, &mut packed);
    vec![packed]
}

/// precomputes the bounding box for each triangle; required for quickly calculating tree splits.
/// result is an array of size tris.length * 6 where triangle i maps to a
/// [x_center, x_delta, y_center, y_delta, z_center, z_delta] tuple starting at index i * 6,
/// representing the center and half-extent in each dimension of triangle i
fn compute_triangle_bounds(positions: &[[f32; 3]], index: &[u32]) -> Vec<f32> {
    let tri_count = index.len() / 3;
    let mut triangle_bounds = vec![0.0_f32; tri_count * 6];
    for tri in 0..tri_count {
        let tri3 = tri * 3;
        let tri6 = tri * 6;
        let ai = index[tri3] as usize;
        let bi = index[tri3 + 1] as usize;
        let ci = index[tri3 + 2] as usize;

        let (pa, pb, pc) = (positions[ai], positions[bi], positions[ci]);
        for el in 0..3 {
            let a = f64::from(pa[el]);
            let b = f64::from(pb[el]);
            let c = f64::from(pc[el]);

            let mut min = a;
            if b < min {
                min = b;
            }
            if c < min {
                min = c;
            }

            let mut max = a;
            if b > max {
                max = b;
            }
            if c > max {
                max = c;
            }

            // Increase the bounds size by float32 epsilon to avoid precision errors when
            // converting to 32 bit float. Scale the epsilon by the size of the numbers being
            // worked with.
            let half_extents = (max - min) / 2.0;
            let el2 = el * 2;
            triangle_bounds[tri6 + el2] = (min + half_extents) as f32;
            triangle_bounds[tri6 + el2 + 1] =
                (half_extents + (min.abs() + half_extents) * FLOAT32_EPSILON) as f32;
        }
    }
    triangle_bounds
}

/// computes the union of the bounds of all of the given triangles and puts the resulting box in "target".
/// A bounding box is computed for the centroids of the triangles, as well, and placed in "centroidTarget".
fn get_bounds(triangle_bounds: &[f32], offset: usize, count: usize) -> ([f32; 6], [f32; 6]) {
    let (mut minx, mut miny, mut minz) = (f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let (mut maxx, mut maxy, mut maxz) = (f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let (mut cminx, mut cminy, mut cminz) = (f64::INFINITY, f64::INFINITY, f64::INFINITY);
    let (mut cmaxx, mut cmaxy, mut cmaxz) =
        (f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);

    let mut i = offset * 6;
    let end = (offset + count) * 6;
    while i < end {
        let cx = f64::from(triangle_bounds[i]);
        let hx = f64::from(triangle_bounds[i + 1]);
        let lx = cx - hx;
        let rx = cx + hx;
        if lx < minx {
            minx = lx;
        }
        if rx > maxx {
            maxx = rx;
        }
        if cx < cminx {
            cminx = cx;
        }
        if cx > cmaxx {
            cmaxx = cx;
        }

        let cy = f64::from(triangle_bounds[i + 2]);
        let hy = f64::from(triangle_bounds[i + 3]);
        let ly = cy - hy;
        let ry = cy + hy;
        if ly < miny {
            miny = ly;
        }
        if ry > maxy {
            maxy = ry;
        }
        if cy < cminy {
            cminy = cy;
        }
        if cy > cmaxy {
            cmaxy = cy;
        }

        let cz = f64::from(triangle_bounds[i + 4]);
        let hz = f64::from(triangle_bounds[i + 5]);
        let lz = cz - hz;
        let rz = cz + hz;
        if lz < minz {
            minz = lz;
        }
        if rz > maxz {
            maxz = rz;
        }
        if cz < cminz {
            cminz = cz;
        }
        if cz > cmaxz {
            cmaxz = cz;
        }

        i += 6;
    }

    // Both targets are Float32Arrays.
    (
        [
            minx as f32,
            miny as f32,
            minz as f32,
            maxx as f32,
            maxy as f32,
            maxz as f32,
        ],
        [
            cminx as f32,
            cminy as f32,
            cminz as f32,
            cmaxx as f32,
            cmaxy as f32,
            cmaxz as f32,
        ],
    )
}

/// `getLongestEdgeIndex`: -1 only when every extent is NaN.
fn get_longest_edge_index(bounds: &[f32; 6]) -> i32 {
    let mut split_dim_idx = -1;
    let mut split_dist = f64::NEG_INFINITY;
    for i in 0..3 {
        let dist = f64::from(bounds[i + 3]) - f64::from(bounds[i]);
        if dist > split_dist {
            split_dist = dist;
            split_dim_idx = i as i32;
        }
    }
    split_dim_idx
}

/// `getOptimalSplit` for `CENTER`: the longest centroid axis, at its middle.
fn get_optimal_split(centroid_bounding_data: &[f32; 6]) -> (i32, f64) {
    let axis = get_longest_edge_index(centroid_bounding_data);
    let mut pos = 0.0;
    if axis != -1 {
        let a = axis as usize;
        pos =
            (f64::from(centroid_bounding_data[a]) + f64::from(centroid_bounding_data[a + 3])) / 2.0;
    }
    (axis, pos)
}

/// reorders `tris` such that for `count` elements after `offset`, elements on the left side of the split
/// will be on the left and elements on the right side of the split will be on the right. returns the index
/// of the first element on the right side, or offset + count if there are no elements on the right side.
fn partition(
    index: &mut [u32],
    triangle_bounds: &mut [f32],
    offset: usize,
    count: usize,
    axis: usize,
    pos: f64,
) -> usize {
    // Signed: `right` may step to offset - 1, which is -1 at offset 0.
    let mut left = offset as isize;
    let mut right = (offset + count) as isize - 1;
    let axis_offset = axis * 2;

    // hoare partitioning, see e.g. https://en.wikipedia.org/wiki/Quicksort#Hoare_partition_scheme
    loop {
        while left <= right && f64::from(triangle_bounds[left as usize * 6 + axis_offset]) < pos {
            left += 1;
        }

        // if a triangle center lies on the partition plane it is considered to be on the right side
        while left <= right && f64::from(triangle_bounds[right as usize * 6 + axis_offset]) >= pos {
            right -= 1;
        }

        if left < right {
            // we need to swap all of the information associated with the triangles at index
            // left and right; that's the verts in the geometry index, the bounds,
            // and perhaps the SAH planes
            let (l, r) = (left as usize, right as usize);
            for i in 0..3 {
                index.swap(l * 3 + i, r * 3 + i);
            }

            // swap bounds
            for i in 0..6 {
                triangle_bounds.swap(l * 6 + i, r * 6 + i);
            }

            left += 1;
            right -= 1;
        } else {
            return left as usize;
        }
    }
}

fn build_tree(
    triangle_bounds: &mut [f32],
    index: &mut [u32],
    offset: usize,
    count: usize,
    options: MeshBvhOptions,
) -> BuildNode {
    let (bounds, centroid) = get_bounds(triangle_bounds, offset, count);
    split_node(
        triangle_bounds,
        index,
        bounds,
        offset,
        count,
        &centroid,
        0,
        options,
    )
}

// either recursively splits the given node, creating left and right subtrees for it, or makes it a leaf node,
// recording the offset and count of its triangles and writing them into the reordered geometry index.
#[allow(clippy::too_many_arguments)]
fn split_node(
    triangle_bounds: &mut [f32],
    index: &mut [u32],
    bounds: [f32; 6],
    offset: usize,
    count: usize,
    centroid_bounding_data: &[f32; 6],
    depth: u32,
    options: MeshBvhOptions,
) -> BuildNode {
    // early out if we've met our capacity
    if count <= options.max_leaf_tris || depth >= options.max_depth {
        return BuildNode::Leaf {
            bounds,
            offset,
            count,
        };
    }

    // Find where to split the volume
    let (axis, pos) = get_optimal_split(centroid_bounding_data);
    if axis == -1 {
        return BuildNode::Leaf {
            bounds,
            offset,
            count,
        };
    }

    let split_offset = partition(index, triangle_bounds, offset, count, axis as usize, pos);

    // create the two new child nodes
    if split_offset == offset || split_offset == offset + count {
        return BuildNode::Leaf {
            bounds,
            offset,
            count,
        };
    }

    // create the left child and compute its bounding box
    let lstart = offset;
    let lcount = split_offset - offset;
    let (left_bounds, left_centroid) = get_bounds(triangle_bounds, lstart, lcount);
    let left = split_node(
        triangle_bounds,
        index,
        left_bounds,
        lstart,
        lcount,
        &left_centroid,
        depth + 1,
        options,
    );

    // repeat for right
    let rstart = split_offset;
    let rcount = count - lcount;
    let (right_bounds, right_centroid) = get_bounds(triangle_bounds, rstart, rcount);
    let right = split_node(
        triangle_bounds,
        index,
        right_bounds,
        rstart,
        rcount,
        &right_centroid,
        depth + 1,
        options,
    );

    BuildNode::Inner {
        bounds,
        split_axis: axis as u32,
        left: Box::new(left),
        right: Box::new(right),
    }
}

/// `populateBuffer`: depth first, left subtree before the right one, so a left
/// child always follows its parent.
fn populate_buffer(node: &BuildNode, out: &mut Vec<Node>) {
    match node {
        BuildNode::Leaf {
            bounds,
            offset,
            count,
        } => out.push(Node {
            bounds: *bounds,
            kind: NodeKind::Leaf {
                offset: u32::try_from(*offset).expect("MeshBVH: triangle offset past 32 bits"),
                // A Uint16Array element: a leaf past 65535 triangles wraps, as in three-mesh-bvh.
                count: (*count & 0xFFFF) as u16,
            },
        }),
        BuildNode::Inner {
            bounds,
            split_axis,
            left,
            right,
        } => {
            let at = out.len();
            out.push(Node {
                bounds: *bounds,
                kind: NodeKind::Leaf {
                    offset: 0,
                    count: 0,
                },
            });
            populate_buffer(left, out);
            let right_at = out.len();
            out[at].kind = NodeKind::Inner {
                right: right_at,
                split_axis: *split_axis,
            };
            populate_buffer(right, out);
        }
    }
}
