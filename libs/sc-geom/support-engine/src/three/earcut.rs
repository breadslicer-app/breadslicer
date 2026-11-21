//! mapbox/earcut 3.0.2 (ISC), the copy three.js r181 ships as
//! `src/extras/lib/earcut.js` behind `Earcut.triangulate`.
//!
//! The doubly linked rings (`prev`/`next`, and the z-order `prevZ`/`nextZ`)
//! live in an arena; a node is its index, and JavaScript's object identity
//! (`p === q`) is index equality. Removed nodes stay in the arena, as removed
//! objects stay reachable from their neighbours' stale pointers in the
//! JavaScript.

#[derive(Debug, Clone)]
struct Node {
    i: usize,
    x: f64,
    y: f64,
    prev: usize,
    next: usize,
    z: i32,
    prev_z: Option<usize>,
    next_z: Option<usize>,
    steiner: bool,
}

struct Earcut<'a> {
    nodes: Vec<Node>,
    triangles: Vec<usize>,
    data: &'a [f64],
}

/// `earcut(data, holeIndices, dim)`: triangle vertex indices, three per
/// triangle.
pub fn earcut(data: &[f64], hole_indices: &[usize], dim: usize) -> Vec<usize> {
    let has_holes = !hole_indices.is_empty();
    let outer_len = if has_holes {
        hole_indices[0] * dim
    } else {
        data.len()
    };
    let mut ec = Earcut {
        nodes: Vec::new(),
        triangles: Vec::new(),
        data,
    };
    let Some(mut outer_node) = ec.linked_list(0, outer_len, dim, true) else {
        return Vec::new();
    };
    if ec.nodes[outer_node].next == ec.nodes[outer_node].prev {
        return Vec::new();
    }
    let (mut min_x, mut min_y, mut inv_size) = (0.0, 0.0, 0.0);
    if has_holes {
        outer_node = ec.eliminate_holes(hole_indices, outer_node, dim);
    }
    // `minX`/`minY`/`invSize` stay undefined below 80 points; `invSize` 0
    // takes the same (unhashed) branches as undefined.
    if data.len() > 80 * dim {
        min_x = data[0];
        min_y = data[1];
        let (mut max_x, mut max_y) = (min_x, min_y);
        let mut i = dim;
        while i < outer_len {
            let (x, y) = (data[i], data[i + 1]);
            if x < min_x {
                min_x = x;
            }
            if y < min_y {
                min_y = y;
            }
            if x > max_x {
                max_x = x;
            }
            if y > max_y {
                max_y = y;
            }
            i += dim;
        }
        inv_size = js_max(max_x - min_x, max_y - min_y);
        inv_size = if inv_size != 0.0 {
            32767.0 / inv_size
        } else {
            0.0
        };
    }
    ec.earcut_linked(Some(outer_node), dim, min_x, min_y, inv_size, 0);
    ec.triangles
}

/// `Math.max` of two: NaN wins.
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

/// `x | 0`: ToInt32.
fn to_int32(a: f64) -> i32 {
    if !a.is_finite() {
        return 0;
    }
    a.trunc().rem_euclid(4_294_967_296.0) as u32 as i32
}

fn signed_area(data: &[f64], start: usize, end: usize, dim: usize) -> f64 {
    let mut sum = 0.0;
    let mut i = start;
    let mut j = end.wrapping_sub(dim);
    while i < end {
        sum += (data[j] - data[i]) * (data[i + 1] + data[j + 1]);
        j = i;
        i += dim;
    }
    sum
}

#[allow(clippy::too_many_arguments)]
fn point_in_triangle(
    ax: f64,
    ay: f64,
    bx: f64,
    by: f64,
    cx: f64,
    cy: f64,
    px: f64,
    py: f64,
) -> bool {
    (cx - px) * (ay - py) >= (ax - px) * (cy - py)
        && (ax - px) * (by - py) >= (bx - px) * (ay - py)
        && (bx - px) * (cy - py) >= (cx - px) * (by - py)
}

#[allow(clippy::too_many_arguments)]
fn point_in_triangle_except_first(
    ax: f64,
    ay: f64,
    bx: f64,
    by: f64,
    cx: f64,
    cy: f64,
    px: f64,
    py: f64,
) -> bool {
    !(ax == px && ay == py) && point_in_triangle(ax, ay, bx, by, cx, cy, px, py)
}

fn z_order(x: f64, y: f64, min_x: f64, min_y: f64, inv_size: f64) -> i32 {
    let mut x = to_int32((x - min_x) * inv_size);
    let mut y = to_int32((y - min_y) * inv_size);
    x = (x | (x << 8)) & 0x00FF00FF;
    x = (x | (x << 4)) & 0x0F0F0F0F;
    x = (x | (x << 2)) & 0x33333333;
    x = (x | (x << 1)) & 0x55555555;
    y = (y | (y << 8)) & 0x00FF00FF;
    y = (y | (y << 4)) & 0x0F0F0F0F;
    y = (y | (y << 2)) & 0x33333333;
    y = (y | (y << 1)) & 0x55555555;
    x | (y << 1)
}

fn sign(num: f64) -> i32 {
    if num > 0.0 {
        1
    } else if num < 0.0 {
        -1
    } else {
        0
    }
}

impl Earcut<'_> {
    fn x(&self, p: usize) -> f64 {
        self.nodes[p].x
    }
    fn y(&self, p: usize) -> f64 {
        self.nodes[p].y
    }
    fn next(&self, p: usize) -> usize {
        self.nodes[p].next
    }
    fn prev(&self, p: usize) -> usize {
        self.nodes[p].prev
    }

    fn create_node(&mut self, i: usize, x: f64, y: f64) -> usize {
        self.nodes.push(Node {
            i,
            x,
            y,
            prev: usize::MAX,
            next: usize::MAX,
            z: 0,
            prev_z: None,
            next_z: None,
            steiner: false,
        });
        self.nodes.len() - 1
    }

    fn insert_node(&mut self, i: usize, x: f64, y: f64, last: Option<usize>) -> usize {
        let p = self.create_node(i, x, y);
        match last {
            None => {
                self.nodes[p].prev = p;
                self.nodes[p].next = p;
            }
            Some(last) => {
                let ln = self.next(last);
                self.nodes[p].next = ln;
                self.nodes[p].prev = last;
                self.nodes[ln].prev = p;
                self.nodes[last].next = p;
            }
        }
        p
    }

    fn remove_node(&mut self, p: usize) {
        let (pn, pp) = (self.next(p), self.prev(p));
        self.nodes[pn].prev = pp;
        self.nodes[pp].next = pn;
        let (pz, nz) = (self.nodes[p].prev_z, self.nodes[p].next_z);
        if let Some(pz) = pz {
            self.nodes[pz].next_z = nz;
        }
        if let Some(nz) = nz {
            self.nodes[nz].prev_z = pz;
        }
    }

    fn equals(&self, p1: usize, p2: usize) -> bool {
        self.x(p1) == self.x(p2) && self.y(p1) == self.y(p2)
    }

    fn area(&self, p: usize, q: usize, r: usize) -> f64 {
        (self.y(q) - self.y(p)) * (self.x(r) - self.x(q))
            - (self.x(q) - self.x(p)) * (self.y(r) - self.y(q))
    }

    fn linked_list(
        &mut self,
        start: usize,
        end: usize,
        dim: usize,
        clockwise: bool,
    ) -> Option<usize> {
        let mut last = None;
        if clockwise == (signed_area(self.data, start, end, dim) > 0.0) {
            let mut i = start;
            while i < end {
                last = Some(self.insert_node(i / dim, self.data[i], self.data[i + 1], last));
                i += dim;
            }
        } else if end >= start + dim {
            let mut i = end - dim;
            loop {
                last = Some(self.insert_node(i / dim, self.data[i], self.data[i + 1], last));
                if i < start + dim {
                    break;
                }
                i -= dim;
            }
        }
        if let Some(l) = last
            && self.equals(l, self.next(l))
        {
            self.remove_node(l);
            last = Some(self.next(l));
        }
        last
    }

    fn filter_points(&mut self, start: Option<usize>, end: Option<usize>) -> Option<usize> {
        let start = start?;
        let mut end = end.unwrap_or(start);
        let mut p = start;
        loop {
            let mut again = false;
            if !self.nodes[p].steiner
                && (self.equals(p, self.next(p)) || self.area(self.prev(p), p, self.next(p)) == 0.0)
            {
                self.remove_node(p);
                p = self.prev(p);
                end = p;
                if p == self.next(p) {
                    break;
                }
                again = true;
            } else {
                p = self.next(p);
            }
            if !(again || p != end) {
                break;
            }
        }
        Some(end)
    }

    fn earcut_linked(
        &mut self,
        ear: Option<usize>,
        dim: usize,
        min_x: f64,
        min_y: f64,
        inv_size: f64,
        pass: u8,
    ) {
        let Some(mut ear) = ear else { return };
        if pass == 0 && truthy(inv_size) {
            self.index_curve(ear, min_x, min_y, inv_size);
        }
        let mut stop = ear;
        while self.prev(ear) != self.next(ear) {
            let prev = self.prev(ear);
            let next = self.next(ear);
            let is_ear = if truthy(inv_size) {
                self.is_ear_hashed(ear, min_x, min_y, inv_size)
            } else {
                self.is_ear(ear)
            };
            if is_ear {
                self.triangles
                    .extend([self.nodes[prev].i, self.nodes[ear].i, self.nodes[next].i]);
                self.remove_node(ear);
                ear = self.next(next);
                stop = self.next(next);
                continue;
            }
            ear = next;
            if ear == stop {
                if pass == 0 {
                    let f = self.filter_points(Some(ear), None);
                    self.earcut_linked(f, dim, min_x, min_y, inv_size, 1);
                } else if pass == 1 {
                    let f = self.filter_points(Some(ear), None);
                    let e = self.cure_local_intersections(f.unwrap());
                    self.earcut_linked(e, dim, min_x, min_y, inv_size, 2);
                } else if pass == 2 {
                    self.split_earcut(ear, dim, min_x, min_y, inv_size);
                }
                break;
            }
        }
    }

    fn is_ear(&self, ear: usize) -> bool {
        let (a, b, c) = (self.prev(ear), ear, self.next(ear));
        if self.area(a, b, c) >= 0.0 {
            return false;
        }
        let (ax, bx, cx, ay, by, cy) = (
            self.x(a),
            self.x(b),
            self.x(c),
            self.y(a),
            self.y(b),
            self.y(c),
        );
        let x0 = ax.min(bx).min(cx);
        let y0 = ay.min(by).min(cy);
        let x1 = ax.max(bx).max(cx);
        let y1 = ay.max(by).max(cy);
        let mut p = self.next(c);
        while p != a {
            let (px, py) = (self.x(p), self.y(p));
            if px >= x0
                && px <= x1
                && py >= y0
                && py <= y1
                && point_in_triangle_except_first(ax, ay, bx, by, cx, cy, px, py)
                && self.area(self.prev(p), p, self.next(p)) >= 0.0
            {
                return false;
            }
            p = self.next(p);
        }
        true
    }

    fn hashed_blocks(
        &self,
        p: usize,
        (a, c): (usize, usize),
        (ax, ay, bx, by, cx, cy): (f64, f64, f64, f64, f64, f64),
        (x0, y0, x1, y1): (f64, f64, f64, f64),
    ) -> bool {
        let (px, py) = (self.x(p), self.y(p));
        px >= x0
            && px <= x1
            && py >= y0
            && py <= y1
            && p != a
            && p != c
            && point_in_triangle_except_first(ax, ay, bx, by, cx, cy, px, py)
            && self.area(self.prev(p), p, self.next(p)) >= 0.0
    }

    fn is_ear_hashed(&self, ear: usize, min_x: f64, min_y: f64, inv_size: f64) -> bool {
        let (a, b, c) = (self.prev(ear), ear, self.next(ear));
        if self.area(a, b, c) >= 0.0 {
            return false;
        }
        let (ax, bx, cx, ay, by, cy) = (
            self.x(a),
            self.x(b),
            self.x(c),
            self.y(a),
            self.y(b),
            self.y(c),
        );
        let x0 = ax.min(bx).min(cx);
        let y0 = ay.min(by).min(cy);
        let x1 = ax.max(bx).max(cx);
        let y1 = ay.max(by).max(cy);
        let min_z = z_order(x0, y0, min_x, min_y, inv_size);
        let max_z = z_order(x1, y1, min_x, min_y, inv_size);
        let tri = (ax, ay, bx, by, cx, cy);
        let bbox = (x0, y0, x1, y1);
        let mut p = self.nodes[ear].prev_z;
        let mut n = self.nodes[ear].next_z;
        while let (Some(pp), Some(nn)) = (p, n) {
            if !(self.nodes[pp].z >= min_z && self.nodes[nn].z <= max_z) {
                break;
            }
            if self.hashed_blocks(pp, (a, c), tri, bbox) {
                return false;
            }
            p = self.nodes[pp].prev_z;
            if self.hashed_blocks(nn, (a, c), tri, bbox) {
                return false;
            }
            n = self.nodes[nn].next_z;
        }
        while let Some(pp) = p {
            if self.nodes[pp].z < min_z {
                break;
            }
            if self.hashed_blocks(pp, (a, c), tri, bbox) {
                return false;
            }
            p = self.nodes[pp].prev_z;
        }
        while let Some(nn) = n {
            if self.nodes[nn].z > max_z {
                break;
            }
            if self.hashed_blocks(nn, (a, c), tri, bbox) {
                return false;
            }
            n = self.nodes[nn].next_z;
        }
        true
    }

    fn cure_local_intersections(&mut self, start: usize) -> Option<usize> {
        let mut start = start;
        let mut p = start;
        loop {
            let a = self.prev(p);
            let b = self.next(self.next(p));
            if !self.equals(a, b)
                && self.intersects(a, p, self.next(p), b)
                && self.locally_inside(a, b)
                && self.locally_inside(b, a)
            {
                self.triangles
                    .extend([self.nodes[a].i, self.nodes[p].i, self.nodes[b].i]);
                self.remove_node(p);
                let pn = self.next(p);
                self.remove_node(pn);
                p = b;
                start = b;
            }
            p = self.next(p);
            if p == start {
                break;
            }
        }
        self.filter_points(Some(p), None)
    }

    fn split_earcut(&mut self, start: usize, dim: usize, min_x: f64, min_y: f64, inv_size: f64) {
        let mut a = start;
        loop {
            let mut b = self.next(self.next(a));
            while b != self.prev(a) {
                if self.nodes[a].i != self.nodes[b].i && self.is_valid_diagonal(a, b) {
                    let c = self.split_polygon(a, b);
                    let an = self.next(a);
                    let a = self.filter_points(Some(a), Some(an));
                    let cn = self.next(c);
                    let c = self.filter_points(Some(c), Some(cn));
                    self.earcut_linked(a, dim, min_x, min_y, inv_size, 0);
                    self.earcut_linked(c, dim, min_x, min_y, inv_size, 0);
                    return;
                }
                b = self.next(b);
            }
            a = self.next(a);
            if a == start {
                break;
            }
        }
    }

    fn eliminate_holes(
        &mut self,
        hole_indices: &[usize],
        mut outer_node: usize,
        dim: usize,
    ) -> usize {
        let mut queue = Vec::new();
        let len = hole_indices.len();
        for i in 0..len {
            let start = hole_indices[i] * dim;
            let end = if i < len - 1 {
                hole_indices[i + 1] * dim
            } else {
                self.data.len()
            };
            // An empty hole ring makes `list` undefined, and the JavaScript
            // throws on `list.next`.
            let list = self
                .linked_list(start, end, dim, false)
                .expect("earcut: empty hole ring");
            if list == self.next(list) {
                self.nodes[list].steiner = true;
            }
            queue.push(self.get_leftmost(list));
        }
        v8_sort(&mut queue, |&a, &b| self.compare_xy_slope(a, b));
        for &hole in &queue {
            outer_node = self.eliminate_hole(hole, outer_node);
        }
        outer_node
    }

    fn compare_xy_slope(&self, a: usize, b: usize) -> f64 {
        let mut result = self.x(a) - self.x(b);
        if result == 0.0 {
            result = self.y(a) - self.y(b);
            if result == 0.0 {
                let (an, bn) = (self.next(a), self.next(b));
                let a_slope = (self.y(an) - self.y(a)) / (self.x(an) - self.x(a));
                let b_slope = (self.y(bn) - self.y(b)) / (self.x(bn) - self.x(b));
                result = a_slope - b_slope;
            }
        }
        result
    }

    fn eliminate_hole(&mut self, hole: usize, outer_node: usize) -> usize {
        let Some(bridge) = self.find_hole_bridge(hole, outer_node) else {
            return outer_node;
        };
        let bridge_reverse = self.split_polygon(bridge, hole);
        let brn = self.next(bridge_reverse);
        self.filter_points(Some(bridge_reverse), Some(brn));
        let bn = self.next(bridge);
        self.filter_points(Some(bridge), Some(bn)).unwrap()
    }

    fn find_hole_bridge(&self, hole: usize, outer_node: usize) -> Option<usize> {
        let mut p = outer_node;
        let (hx, hy) = (self.x(hole), self.y(hole));
        let mut qx = f64::NEG_INFINITY;
        let mut m: Option<usize> = None;
        if self.equals(hole, p) {
            return Some(p);
        }
        loop {
            let pn = self.next(p);
            if self.equals(hole, pn) {
                return Some(pn);
            } else if hy <= self.y(p) && hy >= self.y(pn) && self.y(pn) != self.y(p) {
                let x = self.x(p)
                    + (hy - self.y(p)) * (self.x(pn) - self.x(p)) / (self.y(pn) - self.y(p));
                if x <= hx && x > qx {
                    qx = x;
                    let mm = if self.x(p) < self.x(pn) { p } else { pn };
                    m = Some(mm);
                    if x == hx {
                        return Some(mm);
                    }
                }
            }
            p = pn;
            if p == outer_node {
                break;
            }
        }
        let mut m = m?;
        let stop = m;
        let (mx, my) = (self.x(m), self.y(m));
        let mut tan_min = f64::INFINITY;
        p = m;
        loop {
            let (px, py) = (self.x(p), self.y(p));
            if hx >= px
                && px >= mx
                && hx != px
                && point_in_triangle(
                    if hy < my { hx } else { qx },
                    hy,
                    mx,
                    my,
                    if hy < my { qx } else { hx },
                    hy,
                    px,
                    py,
                )
            {
                let tan = (hy - py).abs() / (hx - px);
                if self.locally_inside(p, hole)
                    && (tan < tan_min
                        || (tan == tan_min
                            && (px > self.x(m)
                                || (px == self.x(m) && self.sector_contains_sector(m, p)))))
                {
                    m = p;
                    tan_min = tan;
                }
            }
            p = self.next(p);
            if p == stop {
                break;
            }
        }
        Some(m)
    }

    fn sector_contains_sector(&self, m: usize, p: usize) -> bool {
        self.area(self.prev(m), m, self.prev(p)) < 0.0
            && self.area(self.next(p), m, self.next(m)) < 0.0
    }

    fn index_curve(&mut self, start: usize, min_x: f64, min_y: f64, inv_size: f64) {
        let mut p = start;
        loop {
            if self.nodes[p].z == 0 {
                self.nodes[p].z = z_order(self.x(p), self.y(p), min_x, min_y, inv_size);
            }
            self.nodes[p].prev_z = Some(self.prev(p));
            self.nodes[p].next_z = Some(self.next(p));
            p = self.next(p);
            if p == start {
                break;
            }
        }
        let pz = self.nodes[p].prev_z.unwrap();
        self.nodes[pz].next_z = None;
        self.nodes[p].prev_z = None;
        self.sort_linked(p);
    }

    fn sort_linked(&mut self, list: usize) {
        let mut list = Some(list);
        let mut in_size = 1;
        loop {
            let mut p = list;
            list = None;
            let mut tail: Option<usize> = None;
            let mut num_merges = 0;
            while let Some(pp) = p {
                num_merges += 1;
                let mut q = Some(pp);
                let mut p_size = 0;
                for _ in 0..in_size {
                    p_size += 1;
                    q = self.nodes[q.unwrap()].next_z;
                    if q.is_none() {
                        break;
                    }
                }
                let mut q_size = in_size;
                let mut p_cur = Some(pp);
                while p_size > 0 || (q_size > 0 && q.is_some()) {
                    let e;
                    if p_size != 0
                        && (q_size == 0
                            || q.is_none()
                            || self.nodes[p_cur.unwrap()].z <= self.nodes[q.unwrap()].z)
                    {
                        e = p_cur.unwrap();
                        p_cur = self.nodes[e].next_z;
                        p_size -= 1;
                    } else {
                        e = q.unwrap();
                        q = self.nodes[e].next_z;
                        q_size -= 1;
                    }
                    match tail {
                        Some(t) => self.nodes[t].next_z = Some(e),
                        None => list = Some(e),
                    }
                    self.nodes[e].prev_z = tail;
                    tail = Some(e);
                }
                p = q;
            }
            self.nodes[tail.unwrap()].next_z = None;
            in_size *= 2;
            if num_merges <= 1 {
                break;
            }
        }
    }

    fn get_leftmost(&self, start: usize) -> usize {
        let mut p = start;
        let mut leftmost = start;
        loop {
            if self.x(p) < self.x(leftmost)
                || (self.x(p) == self.x(leftmost) && self.y(p) < self.y(leftmost))
            {
                leftmost = p;
            }
            p = self.next(p);
            if p == start {
                break;
            }
        }
        leftmost
    }

    fn is_valid_diagonal(&self, a: usize, b: usize) -> bool {
        let bi = self.nodes[b].i;
        self.nodes[self.next(a)].i != bi
            && self.nodes[self.prev(a)].i != bi
            && !self.intersects_polygon(a, b)
            && ((self.locally_inside(a, b)
                && self.locally_inside(b, a)
                && self.middle_inside(a, b)
                // `area(...) || area(...)`: truthy when either is non-zero
                // (NaN is falsy).
                && (truthy(self.area(self.prev(a), a, self.prev(b)))
                    || truthy(self.area(a, self.prev(b), b))))
                || (self.equals(a, b)
                    && self.area(self.prev(a), a, self.next(a)) > 0.0
                    && self.area(self.prev(b), b, self.next(b)) > 0.0))
    }

    fn intersects(&self, p1: usize, q1: usize, p2: usize, q2: usize) -> bool {
        let o1 = sign(self.area(p1, q1, p2));
        let o2 = sign(self.area(p1, q1, q2));
        let o3 = sign(self.area(p2, q2, p1));
        let o4 = sign(self.area(p2, q2, q1));
        if o1 != o2 && o3 != o4 {
            return true;
        }
        if o1 == 0 && self.on_segment(p1, p2, q1) {
            return true;
        }
        if o2 == 0 && self.on_segment(p1, q2, q1) {
            return true;
        }
        if o3 == 0 && self.on_segment(p2, p1, q2) {
            return true;
        }
        if o4 == 0 && self.on_segment(p2, q1, q2) {
            return true;
        }
        false
    }

    fn on_segment(&self, p: usize, q: usize, r: usize) -> bool {
        self.x(q) <= self.x(p).max(self.x(r))
            && self.x(q) >= self.x(p).min(self.x(r))
            && self.y(q) <= self.y(p).max(self.y(r))
            && self.y(q) >= self.y(p).min(self.y(r))
    }

    fn intersects_polygon(&self, a: usize, b: usize) -> bool {
        let (ai, bi) = (self.nodes[a].i, self.nodes[b].i);
        let mut p = a;
        loop {
            let pn = self.next(p);
            let (pi, pni) = (self.nodes[p].i, self.nodes[pn].i);
            if pi != ai && pni != ai && pi != bi && pni != bi && self.intersects(p, pn, a, b) {
                return true;
            }
            p = pn;
            if p == a {
                break;
            }
        }
        false
    }

    fn locally_inside(&self, a: usize, b: usize) -> bool {
        if self.area(self.prev(a), a, self.next(a)) < 0.0 {
            self.area(a, b, self.next(a)) >= 0.0 && self.area(a, self.prev(a), b) >= 0.0
        } else {
            self.area(a, b, self.prev(a)) < 0.0 || self.area(a, self.next(a), b) < 0.0
        }
    }

    fn middle_inside(&self, a: usize, b: usize) -> bool {
        let mut p = a;
        let mut inside = false;
        let px = (self.x(a) + self.x(b)) / 2.0;
        let py = (self.y(a) + self.y(b)) / 2.0;
        loop {
            let pn = self.next(p);
            if ((self.y(p) > py) != (self.y(pn) > py))
                && self.y(pn) != self.y(p)
                && (px
                    < (self.x(pn) - self.x(p)) * (py - self.y(p)) / (self.y(pn) - self.y(p))
                        + self.x(p))
            {
                inside = !inside;
            }
            p = pn;
            if p == a {
                break;
            }
        }
        inside
    }

    fn split_polygon(&mut self, a: usize, b: usize) -> usize {
        let a2 = self.create_node(self.nodes[a].i, self.x(a), self.y(a));
        let b2 = self.create_node(self.nodes[b].i, self.x(b), self.y(b));
        let an = self.next(a);
        let bp = self.prev(b);
        self.nodes[a].next = b;
        self.nodes[b].prev = a;
        self.nodes[a2].next = an;
        self.nodes[an].prev = a2;
        self.nodes[b2].next = a2;
        self.nodes[a2].prev = b2;
        self.nodes[bp].next = b2;
        self.nodes[b2].prev = bp;
        b2
    }
}

fn truthy(v: f64) -> bool {
    v != 0.0 && !v.is_nan()
}

/// `Array.prototype.sort(comparefn)` as V8 runs it: TimSort, whose arrays
/// under 64 elements are one counted run finished by binary insertion. Only
/// `order < 0` is consulted there, so a comparator returning NaN behaves as
/// it does in V8. Longer arrays fall back to Rust's stable sort, the same
/// order whenever the comparator is consistent.
pub fn v8_sort<T: Copy>(a: &mut [T], mut compare: impl FnMut(&T, &T) -> f64) {
    let n = a.len();
    if n < 2 {
        return;
    }
    if n >= 64 {
        a.sort_by(|x, y| {
            let o = compare(x, y);
            if o < 0.0 {
                std::cmp::Ordering::Less
            } else if o > 0.0 {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        });
        return;
    }
    // CountAndMakeRun.
    let mut run = 2;
    let descending = compare(&a[1], &a[0]) < 0.0;
    let mut previous = a[1];
    for &item in &a[2..] {
        let order = compare(&item, &previous);
        if descending {
            if order >= 0.0 {
                break;
            }
        } else if order < 0.0 {
            break;
        }
        previous = item;
        run += 1;
    }
    if descending {
        a[..run].reverse();
    }
    // BinaryInsertionSort(low, startArg = low + run, high).
    for start in run..n {
        let pivot = a[start];
        let (mut left, mut right) = (0, start);
        while left < right {
            let mid = left + ((right - left) >> 1);
            if compare(&pivot, &a[mid]) < 0.0 {
                right = mid;
            } else {
                left = mid + 1;
            }
        }
        let mut p = start;
        while p > left {
            a[p] = a[p - 1];
            p -= 1;
        }
        a[left] = pivot;
    }
}
