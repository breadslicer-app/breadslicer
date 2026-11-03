//! `ClipperBase`, `Clipper` and `PolyTree` of clipper-lib 6.4.2.2, the
//! JavaScript port, transcribed with its linked structures as arena indices.
//! Function names follow the JavaScript in snake case.

use super::{ClipType, IntPoint, Path, Paths, PolyFillType, PolyType, round};

const HORIZONTAL: f64 = -9_007_199_254_740_992.0;
const SKIP: i32 = -2;
const UNASSIGNED: i32 = -1;
const TOLERANCE: f64 = 1e-20;
const LO_RANGE: f64 = 47_453_132.0;
const HI_RANGE: f64 = 4_503_599_627_370_495.0;

/// What clipper-lib throws. In Node `ClipperLib.Error` throws (its `alert`
/// does not exist); `EPrev2` is the JavaScript port's own undefined variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipperError {
    /// `ReferenceError: ePrev2 is not defined` (ProcessEdgesAtTopOfScanbeam).
    EPrev2,
    /// A `ClipperLib.Error(message)`.
    Message(&'static str),
}

pub(crate) fn near_zero(val: f64) -> bool {
    val > -TOLERANCE && val < TOLERANCE
}

fn mul128(a: f64, b: f64) -> i128 {
    (a as i128) * (b as i128)
}

/// `ClipperLib.Cast_Int64` on V8: truncation.
fn cast_int64(a: f64) -> f64 {
    a.trunc()
}

fn slopes_equal4(pt1: IntPoint, pt2: IntPoint, pt3: IntPoint, full_range: bool) -> bool {
    if full_range {
        mul128(pt1.y - pt2.y, pt2.x - pt3.x) == mul128(pt1.x - pt2.x, pt2.y - pt3.y)
    } else {
        cast_int64((pt1.y - pt2.y) * (pt2.x - pt3.x))
            - cast_int64((pt1.x - pt2.x) * (pt2.y - pt3.y))
            == 0.0
    }
}

fn slopes_equal5(
    pt1: IntPoint,
    pt2: IntPoint,
    pt3: IntPoint,
    pt4: IntPoint,
    full_range: bool,
) -> bool {
    if full_range {
        mul128(pt1.y - pt2.y, pt3.x - pt4.x) == mul128(pt1.x - pt2.x, pt3.y - pt4.y)
    } else {
        cast_int64((pt1.y - pt2.y) * (pt3.x - pt4.x))
            - cast_int64((pt1.x - pt2.x) * (pt3.y - pt4.y))
            == 0.0
    }
}

fn pt2_is_between_pt1_and_pt3(pt1: IntPoint, pt2: IntPoint, pt3: IntPoint) -> bool {
    if pt1 == pt3 || pt1 == pt2 || pt3 == pt2 {
        false
    } else if pt1.x != pt3.x {
        (pt2.x > pt1.x) == (pt2.x < pt3.x)
    } else {
        (pt2.y > pt1.y) == (pt2.y < pt3.y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgeSide {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    RightToLeft,
    LeftToRight,
}

#[derive(Debug, Clone)]
struct TEdge {
    bot: IntPoint,
    curr: IntPoint,
    top: IntPoint,
    delta: IntPoint,
    dx: f64,
    poly_typ: PolyType,
    side: EdgeSide,
    wind_delta: i32,
    wind_cnt: i32,
    wind_cnt2: i32,
    out_idx: i32,
    next: usize,
    prev: Option<usize>,
    next_in_lml: Option<usize>,
    next_in_ael: Option<usize>,
    prev_in_ael: Option<usize>,
    next_in_sel: Option<usize>,
    prev_in_sel: Option<usize>,
}

impl TEdge {
    fn new() -> Self {
        TEdge {
            bot: IntPoint::default(),
            curr: IntPoint::default(),
            top: IntPoint::default(),
            delta: IntPoint::default(),
            dx: 0.0,
            poly_typ: PolyType::Subject,
            side: EdgeSide::Left,
            wind_delta: 0,
            wind_cnt: 0,
            wind_cnt2: 0,
            out_idx: 0,
            next: usize::MAX,
            prev: None,
            next_in_lml: None,
            next_in_ael: None,
            prev_in_ael: None,
            next_in_sel: None,
            prev_in_sel: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct IntersectNode {
    edge1: usize,
    edge2: usize,
    pt: IntPoint,
}

#[derive(Debug, Clone, Copy)]
struct LocalMinima {
    y: f64,
    left_bound: Option<usize>,
    right_bound: Option<usize>,
    next: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct Scanbeam {
    y: f64,
    next: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct Maxima {
    x: f64,
    next: Option<usize>,
    prev: Option<usize>,
}

#[derive(Debug, Clone)]
struct OutRec {
    idx: usize,
    is_hole: bool,
    is_open: bool,
    first_left: Option<usize>,
    pts: Option<usize>,
    bottom_pt: Option<usize>,
    poly_node: Option<usize>,
}

#[derive(Debug, Clone, Copy)]
struct OutPt {
    idx: usize,
    pt: IntPoint,
    next: usize,
    prev: usize,
}

#[derive(Debug, Clone, Copy)]
struct Join {
    out_pt1: usize,
    out_pt2: usize,
    off_pt: IntPoint,
}

/// A `PolyNode`: a contour with its children. Node 0 of a [`PolyTree`] is the
/// tree itself.
#[derive(Debug, Clone, Default)]
pub struct PolyNode {
    pub parent: Option<usize>,
    pub polygon: Path,
    pub index: usize,
    pub childs: Vec<usize>,
    pub is_open: bool,
}

/// `ClipperLib.PolyTree`: node 0 is the root.
#[derive(Debug, Clone)]
pub struct PolyTree {
    pub nodes: Vec<PolyNode>,
}

impl Default for PolyTree {
    fn default() -> Self {
        PolyTree {
            nodes: vec![PolyNode::default()],
        }
    }
}

impl PolyTree {
    fn clear(&mut self) {
        self.nodes.truncate(1);
        self.nodes[0].childs.clear();
    }

    fn add_child(&mut self, parent: usize, child: usize) {
        let cnt = self.nodes[parent].childs.len();
        self.nodes[parent].childs.push(child);
        self.nodes[child].parent = Some(parent);
        self.nodes[child].index = cnt;
    }

    /// `IsHoleNode`.
    pub fn is_hole(&self, node: usize) -> bool {
        let mut result = true;
        let mut n = self.nodes[node].parent;
        while let Some(p) = n {
            result = !result;
            n = self.nodes[p].parent;
        }
        result
    }

    /// `PolyTree.GetFirst`.
    pub fn get_first(&self) -> Option<usize> {
        self.nodes[0].childs.first().copied()
    }

    /// `PolyNode.GetNext`: depth first.
    pub fn get_next(&self, node: usize) -> Option<usize> {
        match self.nodes[node].childs.first() {
            Some(&c) => Some(c),
            None => self.get_next_sibling_up(node),
        }
    }

    fn get_next_sibling_up(&self, node: usize) -> Option<usize> {
        let parent = self.nodes[node].parent?;
        if self.nodes[node].index == self.nodes[parent].childs.len() - 1 {
            self.get_next_sibling_up(parent)
        } else {
            Some(self.nodes[parent].childs[self.nodes[node].index + 1])
        }
    }
}

/// `ClipperLib.Clipper` (with `ClipperBase`).
#[derive(Debug, Clone)]
pub struct Clipper {
    edges: Vec<TEdge>,
    minima: Vec<LocalMinima>,
    minima_list: Option<usize>,
    current_lm: Option<usize>,
    use_full_range: bool,
    pub preserve_collinear: bool,
    scanbeams: Vec<Scanbeam>,
    scanbeam: Option<usize>,
    maxima_pool: Vec<Maxima>,
    maxima: Option<usize>,
    out_recs: Vec<OutRec>,
    poly_outs: Vec<usize>,
    out_pts: Vec<OutPt>,
    active_edges: Option<usize>,
    sorted_edges: Option<usize>,
    intersect_list: Vec<IntersectNode>,
    clip_type: ClipType,
    clip_fill_type: PolyFillType,
    subj_fill_type: PolyFillType,
    joins: Vec<Join>,
    ghost_joins: Vec<Join>,
    using_poly_tree: bool,
    pub reverse_solution: bool,
    pub strictly_simple: bool,
}

impl Default for Clipper {
    fn default() -> Self {
        Self::new()
    }
}

impl Clipper {
    pub fn new() -> Self {
        Clipper {
            edges: Vec::new(),
            minima: Vec::new(),
            minima_list: None,
            current_lm: None,
            use_full_range: false,
            preserve_collinear: false,
            scanbeams: Vec::new(),
            scanbeam: None,
            maxima_pool: Vec::new(),
            maxima: None,
            out_recs: Vec::new(),
            poly_outs: Vec::new(),
            out_pts: Vec::new(),
            active_edges: None,
            sorted_edges: None,
            intersect_list: Vec::new(),
            clip_type: ClipType::Intersection,
            clip_fill_type: PolyFillType::EvenOdd,
            subj_fill_type: PolyFillType::EvenOdd,
            joins: Vec::new(),
            ghost_joins: Vec::new(),
            using_poly_tree: false,
            reverse_solution: false,
            strictly_simple: false,
        }
    }

    // ── ClipperBase ────────────────────────────────────────────────

    fn e(&self, i: usize) -> &TEdge {
        &self.edges[i]
    }

    fn em(&mut self, i: usize) -> &mut TEdge {
        &mut self.edges[i]
    }

    fn prev(&self, i: usize) -> usize {
        self.edges[i].prev.expect("a live edge")
    }

    fn is_horizontal(&self, e: usize) -> bool {
        self.edges[e].delta.y == 0.0
    }

    fn slopes_equal3(&self, e1: usize, e2: usize) -> bool {
        let (a, b) = (self.e(e1).delta, self.e(e2).delta);
        if self.use_full_range {
            mul128(a.y, b.x) == mul128(a.x, b.y)
        } else {
            cast_int64(a.y * b.x) == cast_int64(a.x * b.y)
        }
    }

    fn range_test(pt: IntPoint, use_full_range: &mut bool) -> Result<(), ClipperError> {
        if *use_full_range {
            if pt.x > HI_RANGE || pt.y > HI_RANGE || -pt.x > HI_RANGE || -pt.y > HI_RANGE {
                return Err(ClipperError::Message(
                    "Coordinate outside allowed range in RangeTest().",
                ));
            }
        } else if pt.x > LO_RANGE || pt.y > LO_RANGE || -pt.x > LO_RANGE || -pt.y > LO_RANGE {
            *use_full_range = true;
            Self::range_test(pt, use_full_range)?;
        }
        Ok(())
    }

    fn init_edge(&mut self, e: usize, e_next: usize, e_prev: usize, pt: IntPoint) {
        let edge = self.em(e);
        edge.next = e_next;
        edge.prev = Some(e_prev);
        edge.curr = pt;
        edge.out_idx = -1;
    }

    fn init_edge2(&mut self, e: usize, poly_type: PolyType) {
        let next_curr = self.e(self.e(e).next).curr;
        let edge = self.em(e);
        if edge.curr.y >= next_curr.y {
            edge.bot = edge.curr;
            edge.top = next_curr;
        } else {
            edge.top = edge.curr;
            edge.bot = next_curr;
        }
        self.set_dx(e);
        self.em(e).poly_typ = poly_type;
    }

    fn find_next_loc_min(&self, mut e: usize) -> usize {
        loop {
            while self.e(e).bot != self.e(self.prev(e)).bot || self.e(e).curr == self.e(e).top {
                e = self.e(e).next;
            }
            if self.e(e).dx != HORIZONTAL && self.e(self.prev(e)).dx != HORIZONTAL {
                break;
            }
            while self.e(self.prev(e)).dx == HORIZONTAL {
                e = self.prev(e);
            }
            let e2 = e;
            while self.e(e).dx == HORIZONTAL {
                e = self.e(e).next;
            }
            if self.e(e).top.y == self.e(self.prev(e)).bot.y {
                continue;
            }
            if self.e(self.prev(e2)).bot.x < self.e(e).bot.x {
                e = e2;
            }
            break;
        }
        e
    }

    fn process_bound(&mut self, mut e: usize, left_bound_is_forward: bool) -> usize {
        let mut result = e;
        if self.e(result).out_idx == SKIP {
            e = result;
            if left_bound_is_forward {
                while self.e(e).top.y == self.e(self.e(e).next).bot.y {
                    e = self.e(e).next;
                }
                while e != result && self.e(e).dx == HORIZONTAL {
                    e = self.prev(e);
                }
            } else {
                while self.e(e).top.y == self.e(self.prev(e)).bot.y {
                    e = self.prev(e);
                }
                while e != result && self.e(e).dx == HORIZONTAL {
                    e = self.e(e).next;
                }
            }
            if e == result {
                result = if left_bound_is_forward {
                    self.e(e).next
                } else {
                    self.prev(e)
                };
            } else {
                e = if left_bound_is_forward {
                    self.e(result).next
                } else {
                    self.prev(result)
                };
                let lm = self.new_local_minima(self.e(e).bot.y, None, Some(e));
                self.em(e).wind_delta = 0;
                result = self.process_bound(e, left_bound_is_forward);
                self.insert_local_minima(lm);
            }
            return result;
        }
        if self.e(e).dx == HORIZONTAL {
            let e_start = if left_bound_is_forward {
                self.prev(e)
            } else {
                self.e(e).next
            };
            if self.e(e_start).dx == HORIZONTAL {
                if self.e(e_start).bot.x != self.e(e).bot.x
                    && self.e(e_start).top.x != self.e(e).bot.x
                {
                    self.reverse_horizontal(e);
                }
            } else if self.e(e_start).bot.x != self.e(e).bot.x {
                self.reverse_horizontal(e);
            }
        }
        let e_start = e;
        if left_bound_is_forward {
            while self.e(result).top.y == self.e(self.e(result).next).bot.y
                && self.e(self.e(result).next).out_idx != SKIP
            {
                result = self.e(result).next;
            }
            if self.e(result).dx == HORIZONTAL && self.e(self.e(result).next).out_idx != SKIP {
                let mut horz = result;
                while self.e(self.prev(horz)).dx == HORIZONTAL {
                    horz = self.prev(horz);
                }
                if self.e(self.prev(horz)).top.x > self.e(self.e(result).next).top.x {
                    result = self.prev(horz);
                }
            }
            while e != result {
                self.em(e).next_in_lml = Some(self.e(e).next);
                if self.e(e).dx == HORIZONTAL
                    && e != e_start
                    && self.e(e).bot.x != self.e(self.prev(e)).top.x
                {
                    self.reverse_horizontal(e);
                }
                e = self.e(e).next;
            }
            if self.e(e).dx == HORIZONTAL
                && e != e_start
                && self.e(e).bot.x != self.e(self.prev(e)).top.x
            {
                self.reverse_horizontal(e);
            }
            result = self.e(result).next;
        } else {
            while self.e(result).top.y == self.e(self.prev(result)).bot.y
                && self.e(self.prev(result)).out_idx != SKIP
            {
                result = self.prev(result);
            }
            if self.e(result).dx == HORIZONTAL && self.e(self.prev(result)).out_idx != SKIP {
                let mut horz = result;
                while self.e(self.e(horz).next).dx == HORIZONTAL {
                    horz = self.e(horz).next;
                }
                let hn = self.e(horz).next;
                let rp = self.prev(result);
                if self.e(hn).top.x == self.e(rp).top.x || self.e(hn).top.x > self.e(rp).top.x {
                    result = hn;
                }
            }
            while e != result {
                self.em(e).next_in_lml = Some(self.prev(e));
                if self.e(e).dx == HORIZONTAL
                    && e != e_start
                    && self.e(e).bot.x != self.e(self.e(e).next).top.x
                {
                    self.reverse_horizontal(e);
                }
                e = self.prev(e);
            }
            if self.e(e).dx == HORIZONTAL
                && e != e_start
                && self.e(e).bot.x != self.e(self.e(e).next).top.x
            {
                self.reverse_horizontal(e);
            }
            result = self.prev(result);
        }
        result
    }

    fn new_local_minima(&mut self, y: f64, left: Option<usize>, right: Option<usize>) -> usize {
        self.minima.push(LocalMinima {
            y,
            left_bound: left,
            right_bound: right,
            next: None,
        });
        self.minima.len() - 1
    }

    /// `AddPath`. Open paths are not used by DragonFruit's raft and are not
    /// supported here.
    pub fn add_path(
        &mut self,
        pg: &[IntPoint],
        poly_type: PolyType,
        closed: bool,
    ) -> Result<bool, ClipperError> {
        assert!(closed, "open paths are not ported");
        let mut high_i = pg.len() as isize - 1;
        while high_i > 0 && pg[high_i as usize] == pg[0] {
            high_i -= 1;
        }
        while high_i > 0 && pg[high_i as usize] == pg[high_i as usize - 1] {
            high_i -= 1;
        }
        if high_i < 2 {
            return Ok(false);
        }
        let high_i = high_i as usize;
        let base = self.edges.len();
        for _ in 0..=high_i {
            self.edges.push(TEdge::new());
        }
        let ed = |i: usize| base + i;
        let mut is_flat = true;
        self.edges[ed(1)].curr = pg[1];
        let mut full = self.use_full_range;
        Self::range_test(pg[0], &mut full)?;
        self.use_full_range = full;
        Self::range_test(pg[high_i], &mut full)?;
        self.use_full_range = full;
        self.init_edge(ed(0), ed(1), ed(high_i), pg[0]);
        self.init_edge(ed(high_i), ed(0), ed(high_i - 1), pg[high_i]);
        for i in (1..high_i).rev() {
            Self::range_test(pg[i], &mut full)?;
            self.use_full_range = full;
            self.init_edge(ed(i), ed(i + 1), ed(i - 1), pg[i]);
        }

        let mut e_start = ed(0);
        let mut e = e_start;
        let mut e_loop_stop = e_start;
        loop {
            // `E.Curr === E.Next.Curr` compares two distinct point objects in
            // the JavaScript port, so its duplicate removal never fires.
            if self.prev(e) == self.e(e).next {
                break;
            } else if slopes_equal4(
                self.e(self.prev(e)).curr,
                self.e(e).curr,
                self.e(self.e(e).next).curr,
                self.use_full_range,
            ) && (!self.preserve_collinear
                || !pt2_is_between_pt1_and_pt3(
                    self.e(self.prev(e)).curr,
                    self.e(e).curr,
                    self.e(self.e(e).next).curr,
                ))
            {
                if e == e_start {
                    e_start = self.e(e).next;
                }
                e = self.remove_edge(e);
                e = self.prev(e);
                e_loop_stop = e;
                continue;
            }
            e = self.e(e).next;
            if e == e_loop_stop {
                break;
            }
        }
        if self.prev(e) == self.e(e).next {
            return Ok(false);
        }

        e = e_start;
        loop {
            self.init_edge2(e, poly_type);
            e = self.e(e).next;
            if is_flat && self.e(e).curr.y != self.e(e_start).curr.y {
                is_flat = false;
            }
            if e == e_start {
                break;
            }
        }
        if is_flat {
            return Ok(false);
        }

        let mut e_min: Option<usize> = None;
        if self.e(self.prev(e)).bot == self.e(self.prev(e)).top {
            e = self.e(e).next;
        }
        loop {
            e = self.find_next_loc_min(e);
            if Some(e) == e_min {
                break;
            } else if e_min.is_none() {
                e_min = Some(e);
            }
            let left_bound_is_forward;
            let (lb, rb) = if self.e(e).dx < self.e(self.prev(e)).dx {
                left_bound_is_forward = false;
                (self.prev(e), e)
            } else {
                left_bound_is_forward = true;
                (e, self.prev(e))
            };
            let lm = self.new_local_minima(self.e(e).bot.y, Some(lb), Some(rb));
            self.em(lb).side = EdgeSide::Left;
            self.em(rb).side = EdgeSide::Right;
            let lb_wd = if self.e(lb).next == rb { -1 } else { 1 };
            self.em(lb).wind_delta = lb_wd;
            self.em(rb).wind_delta = -lb_wd;
            e = self.process_bound(lb, left_bound_is_forward);
            if self.e(e).out_idx == SKIP {
                e = self.process_bound(e, left_bound_is_forward);
            }
            let mut e2 = self.process_bound(rb, !left_bound_is_forward);
            if self.e(e2).out_idx == SKIP {
                e2 = self.process_bound(e2, !left_bound_is_forward);
            }
            if self.e(lb).out_idx == SKIP {
                self.minima[lm].left_bound = None;
            } else if self.e(rb).out_idx == SKIP {
                self.minima[lm].right_bound = None;
            }
            self.insert_local_minima(lm);
            if !left_bound_is_forward {
                e = e2;
            }
        }
        Ok(true)
    }

    pub fn add_paths(
        &mut self,
        ppg: &[Path],
        poly_type: PolyType,
        closed: bool,
    ) -> Result<bool, ClipperError> {
        let mut result = false;
        for p in ppg {
            if self.add_path(p, poly_type, closed)? {
                result = true;
            }
        }
        Ok(result)
    }

    fn remove_edge(&mut self, e: usize) -> usize {
        let prev = self.prev(e);
        let next = self.e(e).next;
        self.em(prev).next = next;
        self.em(next).prev = Some(prev);
        self.em(e).prev = None;
        next
    }

    fn set_dx(&mut self, e: usize) {
        let edge = self.em(e);
        edge.delta.x = edge.top.x - edge.bot.x;
        edge.delta.y = edge.top.y - edge.bot.y;
        edge.dx = if edge.delta.y == 0.0 {
            HORIZONTAL
        } else {
            edge.delta.x / edge.delta.y
        };
    }

    fn insert_local_minima(&mut self, new_lm: usize) {
        match self.minima_list {
            None => self.minima_list = Some(new_lm),
            Some(head) if self.minima[new_lm].y >= self.minima[head].y => {
                self.minima[new_lm].next = Some(head);
                self.minima_list = Some(new_lm);
            }
            Some(head) => {
                let mut tmp = head;
                while let Some(n) = self.minima[tmp].next {
                    if self.minima[new_lm].y < self.minima[n].y {
                        tmp = n;
                    } else {
                        break;
                    }
                }
                self.minima[new_lm].next = self.minima[tmp].next;
                self.minima[tmp].next = Some(new_lm);
            }
        }
    }

    fn pop_local_minima(&mut self, y: f64) -> Option<usize> {
        let current = self.current_lm?;
        if self.minima[current].y == y {
            self.current_lm = self.minima[current].next;
            Some(current)
        } else {
            None
        }
    }

    fn reverse_horizontal(&mut self, e: usize) {
        let edge = self.em(e);
        std::mem::swap(&mut edge.top.x, &mut edge.bot.x);
    }

    fn reset(&mut self) {
        self.current_lm = self.minima_list;
        if self.current_lm.is_none() {
            return;
        }
        self.scanbeam = None;
        let mut lm = self.minima_list;
        while let Some(l) = lm {
            self.insert_scanbeam(self.minima[l].y);
            for e in [self.minima[l].left_bound, self.minima[l].right_bound]
                .into_iter()
                .flatten()
            {
                let edge = self.em(e);
                edge.curr = edge.bot;
                edge.out_idx = UNASSIGNED;
            }
            lm = self.minima[l].next;
        }
        self.active_edges = None;
    }

    fn insert_scanbeam(&mut self, y: f64) {
        match self.scanbeam {
            None => {
                self.scanbeams.push(Scanbeam { y, next: None });
                self.scanbeam = Some(self.scanbeams.len() - 1);
            }
            Some(head) if y > self.scanbeams[head].y => {
                self.scanbeams.push(Scanbeam {
                    y,
                    next: Some(head),
                });
                self.scanbeam = Some(self.scanbeams.len() - 1);
            }
            Some(head) => {
                let mut sb2 = head;
                while let Some(n) = self.scanbeams[sb2].next {
                    if y <= self.scanbeams[n].y {
                        sb2 = n;
                    } else {
                        break;
                    }
                }
                if y == self.scanbeams[sb2].y {
                    return;
                }
                self.scanbeams.push(Scanbeam {
                    y,
                    next: self.scanbeams[sb2].next,
                });
                let new = self.scanbeams.len() - 1;
                self.scanbeams[sb2].next = Some(new);
            }
        }
    }

    fn pop_scanbeam(&mut self) -> Option<f64> {
        let head = self.scanbeam?;
        self.scanbeam = self.scanbeams[head].next;
        Some(self.scanbeams[head].y)
    }

    fn create_out_rec(&mut self) -> usize {
        let id = self.out_recs.len();
        self.out_recs.push(OutRec {
            idx: self.poly_outs.len(),
            is_hole: false,
            is_open: false,
            first_left: None,
            pts: None,
            bottom_pt: None,
            poly_node: None,
        });
        self.poly_outs.push(id);
        id
    }

    fn update_edge_into_ael(&mut self, e: usize) -> usize {
        let next_lml = self
            .e(e)
            .next_in_lml
            .expect("UpdateEdgeIntoAEL: invalid call");
        let ael_prev = self.e(e).prev_in_ael;
        let ael_next = self.e(e).next_in_ael;
        self.em(next_lml).out_idx = self.e(e).out_idx;
        match ael_prev {
            Some(p) => self.em(p).next_in_ael = Some(next_lml),
            None => self.active_edges = Some(next_lml),
        }
        if let Some(n) = ael_next {
            self.em(n).prev_in_ael = Some(next_lml);
        }
        let (side, wd, wc, wc2) = (
            self.e(e).side,
            self.e(e).wind_delta,
            self.e(e).wind_cnt,
            self.e(e).wind_cnt2,
        );
        let edge = self.em(next_lml);
        edge.side = side;
        edge.wind_delta = wd;
        edge.wind_cnt = wc;
        edge.wind_cnt2 = wc2;
        edge.curr = edge.bot;
        edge.prev_in_ael = ael_prev;
        edge.next_in_ael = ael_next;
        if !self.is_horizontal(next_lml) {
            self.insert_scanbeam(self.e(next_lml).top.y);
        }
        next_lml
    }

    fn swap_positions_in_ael(&mut self, edge1: usize, edge2: usize) {
        if self.e(edge1).next_in_ael == self.e(edge1).prev_in_ael
            || self.e(edge2).next_in_ael == self.e(edge2).prev_in_ael
        {
            return;
        }
        if self.e(edge1).next_in_ael == Some(edge2) {
            let next = self.e(edge2).next_in_ael;
            if let Some(n) = next {
                self.em(n).prev_in_ael = Some(edge1);
            }
            let prev = self.e(edge1).prev_in_ael;
            if let Some(p) = prev {
                self.em(p).next_in_ael = Some(edge2);
            }
            self.em(edge2).prev_in_ael = prev;
            self.em(edge2).next_in_ael = Some(edge1);
            self.em(edge1).prev_in_ael = Some(edge2);
            self.em(edge1).next_in_ael = next;
        } else if self.e(edge2).next_in_ael == Some(edge1) {
            let next = self.e(edge1).next_in_ael;
            if let Some(n) = next {
                self.em(n).prev_in_ael = Some(edge2);
            }
            let prev = self.e(edge2).prev_in_ael;
            if let Some(p) = prev {
                self.em(p).next_in_ael = Some(edge1);
            }
            self.em(edge1).prev_in_ael = prev;
            self.em(edge1).next_in_ael = Some(edge2);
            self.em(edge2).prev_in_ael = Some(edge1);
            self.em(edge2).next_in_ael = next;
        } else {
            let next = self.e(edge1).next_in_ael;
            let prev = self.e(edge1).prev_in_ael;
            self.em(edge1).next_in_ael = self.e(edge2).next_in_ael;
            if let Some(n) = self.e(edge1).next_in_ael {
                self.em(n).prev_in_ael = Some(edge1);
            }
            self.em(edge1).prev_in_ael = self.e(edge2).prev_in_ael;
            if let Some(p) = self.e(edge1).prev_in_ael {
                self.em(p).next_in_ael = Some(edge1);
            }
            self.em(edge2).next_in_ael = next;
            if let Some(n) = next {
                self.em(n).prev_in_ael = Some(edge2);
            }
            self.em(edge2).prev_in_ael = prev;
            if let Some(p) = prev {
                self.em(p).next_in_ael = Some(edge2);
            }
        }
        if self.e(edge1).prev_in_ael.is_none() {
            self.active_edges = Some(edge1);
        } else if self.e(edge2).prev_in_ael.is_none() {
            self.active_edges = Some(edge2);
        }
    }

    fn delete_from_ael(&mut self, e: usize) {
        let ael_prev = self.e(e).prev_in_ael;
        let ael_next = self.e(e).next_in_ael;
        if ael_prev.is_none() && ael_next.is_none() && Some(e) != self.active_edges {
            return;
        }
        match ael_prev {
            Some(p) => self.em(p).next_in_ael = ael_next,
            None => self.active_edges = ael_next,
        }
        if let Some(n) = ael_next {
            self.em(n).prev_in_ael = ael_prev;
        }
        self.em(e).next_in_ael = None;
        self.em(e).prev_in_ael = None;
    }

    // ── Clipper ────────────────────────────────────────────────────

    fn insert_maxima(&mut self, x: f64) {
        let new_max = self.maxima_pool.len();
        self.maxima_pool.push(Maxima {
            x,
            next: None,
            prev: None,
        });
        match self.maxima {
            None => self.maxima = Some(new_max),
            Some(head) if x < self.maxima_pool[head].x => {
                self.maxima_pool[new_max].next = Some(head);
                self.maxima = Some(new_max);
            }
            Some(head) => {
                let mut m = head;
                while let Some(n) = self.maxima_pool[m].next {
                    if x >= self.maxima_pool[n].x {
                        m = n;
                    } else {
                        break;
                    }
                }
                if x == self.maxima_pool[m].x {
                    return;
                }
                let m_next = self.maxima_pool[m].next;
                self.maxima_pool[new_max].next = m_next;
                self.maxima_pool[new_max].prev = Some(m);
                if let Some(n) = m_next {
                    self.maxima_pool[n].prev = Some(new_max);
                }
                self.maxima_pool[m].next = Some(new_max);
            }
        }
    }

    /// `Execute(clipType, polytree, subjFillType, clipFillType)`.
    pub fn execute_tree(
        &mut self,
        clip_type: ClipType,
        polytree: &mut PolyTree,
        subj_fill_type: PolyFillType,
        clip_fill_type: PolyFillType,
    ) -> Result<bool, ClipperError> {
        self.subj_fill_type = subj_fill_type;
        self.clip_fill_type = clip_fill_type;
        self.clip_type = clip_type;
        self.using_poly_tree = true;
        let result = self.execute_internal();
        if let Ok(true) = result {
            self.build_result2(polytree);
        }
        self.dispose_all_poly_pts();
        result
    }

    /// `Execute(clipType, solution, subjFillType, clipFillType)`.
    pub fn execute_paths(
        &mut self,
        clip_type: ClipType,
        solution: &mut Paths,
        subj_fill_type: PolyFillType,
        clip_fill_type: PolyFillType,
    ) -> Result<bool, ClipperError> {
        solution.clear();
        self.subj_fill_type = subj_fill_type;
        self.clip_fill_type = clip_fill_type;
        self.clip_type = clip_type;
        self.using_poly_tree = false;
        let result = self.execute_internal();
        if let Ok(true) = result {
            self.build_result(solution);
        }
        self.dispose_all_poly_pts();
        result
    }

    fn fix_hole_linkage(&mut self, out_rec: usize) {
        let Some(fl) = self.out_recs[out_rec].first_left else {
            return;
        };
        if self.out_recs[out_rec].is_hole != self.out_recs[fl].is_hole
            && self.out_recs[fl].pts.is_some()
        {
            return;
        }
        let mut orfl = Some(fl);
        while let Some(o) = orfl {
            if self.out_recs[o].is_hole == self.out_recs[out_rec].is_hole
                || self.out_recs[o].pts.is_none()
            {
                orfl = self.out_recs[o].first_left;
            } else {
                break;
            }
        }
        self.out_recs[out_rec].first_left = orfl;
    }

    fn execute_internal(&mut self) -> Result<bool, ClipperError> {
        let result = self.execute_sweep();
        self.joins.clear();
        self.ghost_joins.clear();
        result
    }

    fn execute_sweep(&mut self) -> Result<bool, ClipperError> {
        self.reset();
        self.sorted_edges = None;
        self.maxima = None;
        let Some(bot_y) = self.pop_scanbeam() else {
            return Ok(false);
        };
        self.insert_local_minima_into_ael(bot_y);
        loop {
            let top_y = self.pop_scanbeam();
            if top_y.is_none() && self.current_lm.is_none() {
                break;
            }
            // `PopScanbeam` writes 0 when the list is empty.
            let top_y = top_y.unwrap_or(0.0);
            self.process_horizontals();
            self.ghost_joins.clear();
            if !self.process_intersections(top_y)? {
                return Ok(false);
            }
            self.process_edges_at_top_of_scanbeam(top_y)?;
            self.insert_local_minima_into_ael(top_y);
        }
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            if self.out_recs[out_rec].pts.is_none() || self.out_recs[out_rec].is_open {
                continue;
            }
            if (self.out_recs[out_rec].is_hole ^ self.reverse_solution)
                == (self.area_out_rec(out_rec) > 0.0)
            {
                self.reverse_poly_pt_links(self.out_recs[out_rec].pts);
            }
        }
        self.join_common_edges();
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            if self.out_recs[out_rec].pts.is_none() {
                continue;
            }
            self.fixup_out_polygon(out_rec);
        }
        if self.strictly_simple {
            self.do_simple_polygons();
        }
        Ok(true)
    }

    fn dispose_all_poly_pts(&mut self) {
        for &o in &self.poly_outs {
            self.out_recs[o].pts = None;
        }
        self.poly_outs.clear();
    }

    fn add_join(&mut self, op1: usize, op2: usize, off_pt: IntPoint) {
        self.joins.push(Join {
            out_pt1: op1,
            out_pt2: op2,
            off_pt,
        });
    }

    fn add_ghost_join(&mut self, op: usize, off_pt: IntPoint) {
        self.ghost_joins.push(Join {
            out_pt1: op,
            out_pt2: usize::MAX,
            off_pt,
        });
    }

    fn insert_local_minima_into_ael(&mut self, bot_y: f64) {
        while let Some(lm) = self.pop_local_minima(bot_y) {
            let lb = self.minima[lm].left_bound;
            let rb = self.minima[lm].right_bound;
            let mut op1: Option<usize> = None;
            match (lb, rb) {
                (None, Some(rb)) => {
                    self.insert_edge_into_ael(rb, None);
                    self.set_winding_count(rb);
                    if self.is_contributing(rb) {
                        op1 = Some(self.add_out_pt(rb, self.e(rb).bot));
                    }
                }
                (Some(lb), None) => {
                    self.insert_edge_into_ael(lb, None);
                    self.set_winding_count(lb);
                    if self.is_contributing(lb) {
                        op1 = Some(self.add_out_pt(lb, self.e(lb).bot));
                    }
                    self.insert_scanbeam(self.e(lb).top.y);
                }
                (Some(lb), Some(rb)) => {
                    self.insert_edge_into_ael(lb, None);
                    self.insert_edge_into_ael(rb, Some(lb));
                    self.set_winding_count(lb);
                    let (wc, wc2) = (self.e(lb).wind_cnt, self.e(lb).wind_cnt2);
                    self.em(rb).wind_cnt = wc;
                    self.em(rb).wind_cnt2 = wc2;
                    if self.is_contributing(lb) {
                        op1 = Some(self.add_local_min_poly(lb, rb, self.e(lb).bot));
                    }
                    self.insert_scanbeam(self.e(lb).top.y);
                }
                (None, None) => {}
            }
            if let Some(rb) = rb {
                if self.is_horizontal(rb) {
                    if let Some(n) = self.e(rb).next_in_lml {
                        self.insert_scanbeam(self.e(n).top.y);
                    }
                    self.add_edge_to_sel(rb);
                } else {
                    self.insert_scanbeam(self.e(rb).top.y);
                }
            }
            let (Some(lb), Some(rb)) = (lb, rb) else {
                continue;
            };
            if let Some(op1) = op1
                && self.is_horizontal(rb)
                && !self.ghost_joins.is_empty()
                && self.e(rb).wind_delta != 0
            {
                for i in 0..self.ghost_joins.len() {
                    let j = self.ghost_joins[i];
                    if Self::horz_segments_overlap(
                        self.out_pts[j.out_pt1].pt.x,
                        j.off_pt.x,
                        self.e(rb).bot.x,
                        self.e(rb).top.x,
                    ) {
                        self.add_join(j.out_pt1, op1, j.off_pt);
                    }
                }
            }
            if let Some(p) = self.e(lb).prev_in_ael
                && self.e(lb).out_idx >= 0
                && self.e(p).curr.x == self.e(lb).bot.x
                && self.e(p).out_idx >= 0
                && slopes_equal5(
                    self.e(p).curr,
                    self.e(p).top,
                    self.e(lb).curr,
                    self.e(lb).top,
                    self.use_full_range,
                )
                && self.e(lb).wind_delta != 0
                && self.e(p).wind_delta != 0
            {
                let op2 = self.add_out_pt(p, self.e(lb).bot);
                self.add_join(op1.expect("lb contributes"), op2, self.e(lb).top);
            }
            if self.e(lb).next_in_ael != Some(rb) {
                let rp = self.e(rb).prev_in_ael.expect("rb follows lb");
                if self.e(rb).out_idx >= 0
                    && self.e(rp).out_idx >= 0
                    && slopes_equal5(
                        self.e(rp).curr,
                        self.e(rp).top,
                        self.e(rb).curr,
                        self.e(rb).top,
                        self.use_full_range,
                    )
                    && self.e(rb).wind_delta != 0
                    && self.e(rp).wind_delta != 0
                {
                    let op2 = self.add_out_pt(rp, self.e(rb).bot);
                    self.add_join(op1.expect("rb contributes"), op2, self.e(rb).top);
                }
                let mut e = self.e(lb).next_in_ael;
                if e.is_some() {
                    while let Some(ei) = e {
                        if ei == rb {
                            break;
                        }
                        let lb_curr = self.e(lb).curr;
                        self.intersect_edges(rb, ei, lb_curr);
                        e = self.e(ei).next_in_ael;
                    }
                }
            }
        }
    }

    fn insert_edge_into_ael(&mut self, edge: usize, start_edge: Option<usize>) {
        match self.active_edges {
            None => {
                self.em(edge).prev_in_ael = None;
                self.em(edge).next_in_ael = None;
                self.active_edges = Some(edge);
            }
            Some(ae) if start_edge.is_none() && self.e2_inserts_before_e1(ae, edge) => {
                self.em(edge).prev_in_ael = None;
                self.em(edge).next_in_ael = Some(ae);
                self.em(ae).prev_in_ael = Some(edge);
                self.active_edges = Some(edge);
            }
            Some(ae) => {
                let mut start = start_edge.unwrap_or(ae);
                while let Some(n) = self.e(start).next_in_ael {
                    if self.e2_inserts_before_e1(n, edge) {
                        break;
                    }
                    start = n;
                }
                let sn = self.e(start).next_in_ael;
                self.em(edge).next_in_ael = sn;
                if let Some(n) = sn {
                    self.em(n).prev_in_ael = Some(edge);
                }
                self.em(edge).prev_in_ael = Some(start);
                self.em(start).next_in_ael = Some(edge);
            }
        }
    }

    fn e2_inserts_before_e1(&self, e1: usize, e2: usize) -> bool {
        if self.e(e2).curr.x == self.e(e1).curr.x {
            if self.e(e2).top.y > self.e(e1).top.y {
                self.e(e2).top.x < self.top_x(e1, self.e(e2).top.y)
            } else {
                self.e(e1).top.x > self.top_x(e2, self.e(e1).top.y)
            }
        } else {
            self.e(e2).curr.x < self.e(e1).curr.x
        }
    }

    fn is_even_odd_fill_type(&self, edge: usize) -> bool {
        if self.e(edge).poly_typ == PolyType::Subject {
            self.subj_fill_type == PolyFillType::EvenOdd
        } else {
            self.clip_fill_type == PolyFillType::EvenOdd
        }
    }

    fn is_even_odd_alt_fill_type(&self, edge: usize) -> bool {
        if self.e(edge).poly_typ == PolyType::Subject {
            self.clip_fill_type == PolyFillType::EvenOdd
        } else {
            self.subj_fill_type == PolyFillType::EvenOdd
        }
    }

    fn is_contributing(&self, edge: usize) -> bool {
        let e = self.e(edge);
        let (pft, pft2) = if e.poly_typ == PolyType::Subject {
            (self.subj_fill_type, self.clip_fill_type)
        } else {
            (self.clip_fill_type, self.subj_fill_type)
        };
        match pft {
            PolyFillType::EvenOdd => {
                if e.wind_delta == 0 && e.wind_cnt != 1 {
                    return false;
                }
            }
            PolyFillType::NonZero => {
                if e.wind_cnt.abs() != 1 {
                    return false;
                }
            }
            PolyFillType::Positive => {
                if e.wind_cnt != 1 {
                    return false;
                }
            }
            PolyFillType::Negative => {
                if e.wind_cnt != -1 {
                    return false;
                }
            }
        }
        let zero_side = |pft2: PolyFillType| match pft2 {
            PolyFillType::EvenOdd | PolyFillType::NonZero => e.wind_cnt2 == 0,
            PolyFillType::Positive => e.wind_cnt2 <= 0,
            PolyFillType::Negative => e.wind_cnt2 >= 0,
        };
        let inside = |pft2: PolyFillType| match pft2 {
            PolyFillType::EvenOdd | PolyFillType::NonZero => e.wind_cnt2 != 0,
            PolyFillType::Positive => e.wind_cnt2 > 0,
            PolyFillType::Negative => e.wind_cnt2 < 0,
        };
        match self.clip_type {
            ClipType::Intersection => inside(pft2),
            ClipType::Union => zero_side(pft2),
            ClipType::Difference => {
                if e.poly_typ == PolyType::Subject {
                    zero_side(pft2)
                } else {
                    inside(pft2)
                }
            }
            ClipType::Xor => {
                if e.wind_delta == 0 {
                    zero_side(pft2)
                } else {
                    true
                }
            }
        }
    }

    fn set_winding_count(&mut self, edge: usize) {
        let mut e = self.e(edge).prev_in_ael;
        while let Some(ei) = e {
            if self.e(ei).poly_typ != self.e(edge).poly_typ || self.e(ei).wind_delta == 0 {
                e = self.e(ei).prev_in_ael;
            } else {
                break;
            }
        }
        let mut cursor: Option<usize>;
        match e {
            None => {
                let pft = if self.e(edge).poly_typ == PolyType::Subject {
                    self.subj_fill_type
                } else {
                    self.clip_fill_type
                };
                let wd = self.e(edge).wind_delta;
                self.em(edge).wind_cnt = if wd == 0 {
                    if pft == PolyFillType::Negative { -1 } else { 1 }
                } else {
                    wd
                };
                self.em(edge).wind_cnt2 = 0;
                cursor = self.active_edges;
            }
            Some(ei) if self.e(edge).wind_delta == 0 && self.clip_type != ClipType::Union => {
                self.em(edge).wind_cnt = 1;
                self.em(edge).wind_cnt2 = self.e(ei).wind_cnt2;
                cursor = self.e(ei).next_in_ael;
            }
            Some(ei) if self.is_even_odd_fill_type(edge) => {
                if self.e(edge).wind_delta == 0 {
                    let mut inside = true;
                    let mut e2 = self.e(ei).prev_in_ael;
                    while let Some(x) = e2 {
                        if self.e(x).poly_typ == self.e(ei).poly_typ && self.e(x).wind_delta != 0 {
                            inside = !inside;
                        }
                        e2 = self.e(x).prev_in_ael;
                    }
                    self.em(edge).wind_cnt = if inside { 0 } else { 1 };
                } else {
                    self.em(edge).wind_cnt = self.e(edge).wind_delta;
                }
                self.em(edge).wind_cnt2 = self.e(ei).wind_cnt2;
                cursor = self.e(ei).next_in_ael;
            }
            Some(ei) => {
                let (ewc, ewd, wd) = (
                    self.e(ei).wind_cnt,
                    self.e(ei).wind_delta,
                    self.e(edge).wind_delta,
                );
                let wc = if ewc * ewd < 0 {
                    if ewc.abs() > 1 {
                        if ewd * wd < 0 { ewc } else { ewc + wd }
                    } else if wd == 0 {
                        1
                    } else {
                        wd
                    }
                } else if wd == 0 {
                    if ewc < 0 { ewc - 1 } else { ewc + 1 }
                } else if ewd * wd < 0 {
                    ewc
                } else {
                    ewc + wd
                };
                self.em(edge).wind_cnt = wc;
                self.em(edge).wind_cnt2 = self.e(ei).wind_cnt2;
                cursor = self.e(ei).next_in_ael;
            }
        }
        if self.is_even_odd_alt_fill_type(edge) {
            while let Some(c) = cursor {
                if c == edge {
                    break;
                }
                if self.e(c).wind_delta != 0 {
                    let w2 = self.e(edge).wind_cnt2;
                    self.em(edge).wind_cnt2 = if w2 == 0 { 1 } else { 0 };
                }
                cursor = self.e(c).next_in_ael;
            }
        } else {
            while let Some(c) = cursor {
                if c == edge {
                    break;
                }
                let d = self.e(c).wind_delta;
                self.em(edge).wind_cnt2 += d;
                cursor = self.e(c).next_in_ael;
            }
        }
    }

    fn add_edge_to_sel(&mut self, edge: usize) {
        match self.sorted_edges {
            None => {
                self.sorted_edges = Some(edge);
                self.em(edge).prev_in_sel = None;
                self.em(edge).next_in_sel = None;
            }
            Some(s) => {
                self.em(edge).next_in_sel = Some(s);
                self.em(edge).prev_in_sel = None;
                self.em(s).prev_in_sel = Some(edge);
                self.sorted_edges = Some(edge);
            }
        }
    }

    fn pop_edge_from_sel(&mut self) -> Option<usize> {
        let old = self.sorted_edges?;
        self.sorted_edges = self.e(old).next_in_sel;
        if let Some(s) = self.sorted_edges {
            self.em(s).prev_in_sel = None;
        }
        self.em(old).next_in_sel = None;
        self.em(old).prev_in_sel = None;
        Some(old)
    }

    fn copy_ael_to_sel(&mut self) {
        let mut e = self.active_edges;
        self.sorted_edges = e;
        while let Some(ei) = e {
            let (p, n) = (self.e(ei).prev_in_ael, self.e(ei).next_in_ael);
            self.em(ei).prev_in_sel = p;
            self.em(ei).next_in_sel = n;
            e = n;
        }
    }

    fn swap_positions_in_sel(&mut self, edge1: usize, edge2: usize) {
        if self.e(edge1).next_in_sel.is_none() && self.e(edge1).prev_in_sel.is_none() {
            return;
        }
        if self.e(edge2).next_in_sel.is_none() && self.e(edge2).prev_in_sel.is_none() {
            return;
        }
        if self.e(edge1).next_in_sel == Some(edge2) {
            let next = self.e(edge2).next_in_sel;
            if let Some(n) = next {
                self.em(n).prev_in_sel = Some(edge1);
            }
            let prev = self.e(edge1).prev_in_sel;
            if let Some(p) = prev {
                self.em(p).next_in_sel = Some(edge2);
            }
            self.em(edge2).prev_in_sel = prev;
            self.em(edge2).next_in_sel = Some(edge1);
            self.em(edge1).prev_in_sel = Some(edge2);
            self.em(edge1).next_in_sel = next;
        } else if self.e(edge2).next_in_sel == Some(edge1) {
            let next = self.e(edge1).next_in_sel;
            if let Some(n) = next {
                self.em(n).prev_in_sel = Some(edge2);
            }
            let prev = self.e(edge2).prev_in_sel;
            if let Some(p) = prev {
                self.em(p).next_in_sel = Some(edge1);
            }
            self.em(edge1).prev_in_sel = prev;
            self.em(edge1).next_in_sel = Some(edge2);
            self.em(edge2).prev_in_sel = Some(edge1);
            self.em(edge2).next_in_sel = next;
        } else {
            let next = self.e(edge1).next_in_sel;
            let prev = self.e(edge1).prev_in_sel;
            self.em(edge1).next_in_sel = self.e(edge2).next_in_sel;
            if let Some(n) = self.e(edge1).next_in_sel {
                self.em(n).prev_in_sel = Some(edge1);
            }
            self.em(edge1).prev_in_sel = self.e(edge2).prev_in_sel;
            if let Some(p) = self.e(edge1).prev_in_sel {
                self.em(p).next_in_sel = Some(edge1);
            }
            self.em(edge2).next_in_sel = next;
            if let Some(n) = next {
                self.em(n).prev_in_sel = Some(edge2);
            }
            self.em(edge2).prev_in_sel = prev;
            if let Some(p) = prev {
                self.em(p).next_in_sel = Some(edge2);
            }
        }
        if self.e(edge1).prev_in_sel.is_none() {
            self.sorted_edges = Some(edge1);
        } else if self.e(edge2).prev_in_sel.is_none() {
            self.sorted_edges = Some(edge2);
        }
    }

    fn add_local_max_poly(&mut self, e1: usize, e2: usize, pt: IntPoint) {
        self.add_out_pt(e1, pt);
        if self.e(e2).wind_delta == 0 {
            self.add_out_pt(e2, pt);
        }
        if self.e(e1).out_idx == self.e(e2).out_idx {
            self.em(e1).out_idx = -1;
            self.em(e2).out_idx = -1;
        } else if self.e(e1).out_idx < self.e(e2).out_idx {
            self.append_polygon(e1, e2);
        } else {
            self.append_polygon(e2, e1);
        }
    }

    fn add_local_min_poly(&mut self, e1: usize, e2: usize, pt: IntPoint) -> usize {
        let result;
        let e;
        let prev_e = if self.is_horizontal(e2) || self.e(e1).dx > self.e(e2).dx {
            result = self.add_out_pt(e1, pt);
            self.em(e2).out_idx = self.e(e1).out_idx;
            self.em(e1).side = EdgeSide::Left;
            self.em(e2).side = EdgeSide::Right;
            e = e1;
            if self.e(e).prev_in_ael == Some(e2) {
                self.e(e2).prev_in_ael
            } else {
                self.e(e).prev_in_ael
            }
        } else {
            result = self.add_out_pt(e2, pt);
            self.em(e1).out_idx = self.e(e2).out_idx;
            self.em(e1).side = EdgeSide::Right;
            self.em(e2).side = EdgeSide::Left;
            e = e2;
            if self.e(e).prev_in_ael == Some(e1) {
                self.e(e1).prev_in_ael
            } else {
                self.e(e).prev_in_ael
            }
        };
        if let Some(pe) = prev_e
            && self.e(pe).out_idx >= 0
            && self.e(pe).top.y < pt.y
            && self.e(e).top.y < pt.y
        {
            let x_prev = self.top_x(pe, pt.y);
            let x_e = self.top_x(e, pt.y);
            if x_prev == x_e
                && self.e(e).wind_delta != 0
                && self.e(pe).wind_delta != 0
                && slopes_equal5(
                    IntPoint::new(x_prev, pt.y),
                    self.e(pe).top,
                    IntPoint::new(x_e, pt.y),
                    self.e(e).top,
                    self.use_full_range,
                )
            {
                let out_pt = self.add_out_pt(pe, pt);
                self.add_join(result, out_pt, self.e(e).top);
            }
        }
        result
    }

    fn new_out_pt(&mut self, idx: usize, pt: IntPoint) -> usize {
        let id = self.out_pts.len();
        self.out_pts.push(OutPt {
            idx,
            pt,
            next: id,
            prev: id,
        });
        id
    }

    fn add_out_pt(&mut self, e: usize, pt: IntPoint) -> usize {
        if self.e(e).out_idx < 0 {
            let out_rec = self.create_out_rec();
            self.out_recs[out_rec].is_open = self.e(e).wind_delta == 0;
            let idx = self.out_recs[out_rec].idx;
            let new_op = self.new_out_pt(idx, pt);
            self.out_recs[out_rec].pts = Some(new_op);
            if !self.out_recs[out_rec].is_open {
                self.set_hole_state(e, out_rec);
            }
            self.em(e).out_idx = idx as i32;
            new_op
        } else {
            let out_rec = self.poly_outs[self.e(e).out_idx as usize];
            let op = self.out_recs[out_rec].pts.expect("a live OutRec");
            let to_front = self.e(e).side == EdgeSide::Left;
            if to_front && pt == self.out_pts[op].pt {
                return op;
            } else if !to_front && pt == self.out_pts[self.out_pts[op].prev].pt {
                return self.out_pts[op].prev;
            }
            let idx = self.out_recs[out_rec].idx;
            let new_op = self.new_out_pt(idx, pt);
            let op_prev = self.out_pts[op].prev;
            self.out_pts[new_op].next = op;
            self.out_pts[new_op].prev = op_prev;
            self.out_pts[op_prev].next = new_op;
            self.out_pts[op].prev = new_op;
            if to_front {
                self.out_recs[out_rec].pts = Some(new_op);
            }
            new_op
        }
    }

    fn get_last_out_pt(&self, e: usize) -> usize {
        let out_rec = self.poly_outs[self.e(e).out_idx as usize];
        let pts = self.out_recs[out_rec].pts.expect("a live OutRec");
        if self.e(e).side == EdgeSide::Left {
            pts
        } else {
            self.out_pts[pts].prev
        }
    }

    fn horz_segments_overlap(
        mut seg1a: f64,
        mut seg1b: f64,
        mut seg2a: f64,
        mut seg2b: f64,
    ) -> bool {
        if seg1a > seg1b {
            std::mem::swap(&mut seg1a, &mut seg1b);
        }
        if seg2a > seg2b {
            std::mem::swap(&mut seg2a, &mut seg2b);
        }
        seg1a < seg2b && seg2a < seg1b
    }

    fn set_hole_state(&mut self, e: usize, out_rec: usize) {
        let mut e2 = self.e(e).prev_in_ael;
        let mut e_tmp: Option<usize> = None;
        while let Some(x) = e2 {
            if self.e(x).out_idx >= 0 && self.e(x).wind_delta != 0 {
                match e_tmp {
                    None => e_tmp = Some(x),
                    Some(t) if self.e(t).out_idx == self.e(x).out_idx => e_tmp = None,
                    _ => {}
                }
            }
            e2 = self.e(x).prev_in_ael;
        }
        match e_tmp {
            None => {
                self.out_recs[out_rec].first_left = None;
                self.out_recs[out_rec].is_hole = false;
            }
            Some(t) => {
                let fl = self.poly_outs[self.e(t).out_idx as usize];
                self.out_recs[out_rec].first_left = Some(fl);
                self.out_recs[out_rec].is_hole = !self.out_recs[fl].is_hole;
            }
        }
    }

    fn get_dx(pt1: IntPoint, pt2: IntPoint) -> f64 {
        if pt1.y == pt2.y {
            HORIZONTAL
        } else {
            (pt2.x - pt1.x) / (pt2.y - pt1.y)
        }
    }

    fn first_is_bottom_pt(&self, btm_pt1: usize, btm_pt2: usize) -> bool {
        let op = &self.out_pts;
        let walk = |start: usize, forward: bool| {
            let mut p = if forward {
                op[start].next
            } else {
                op[start].prev
            };
            while op[p].pt == op[start].pt && p != start {
                p = if forward { op[p].next } else { op[p].prev };
            }
            Self::get_dx(op[start].pt, op[p].pt).abs()
        };
        let dx1p = walk(btm_pt1, false);
        let dx1n = walk(btm_pt1, true);
        let dx2p = walk(btm_pt2, false);
        let dx2n = walk(btm_pt2, true);
        if dx1p.max(dx1n) == dx2p.max(dx2n) && dx1p.min(dx1n) == dx2p.min(dx2n) {
            self.area_op(Some(btm_pt1)) > 0.0
        } else {
            (dx1p >= dx2p && dx1p >= dx2n) || (dx1n >= dx2p && dx1n >= dx2n)
        }
    }

    fn get_bottom_pt(&self, mut pp: usize) -> usize {
        let op = &self.out_pts;
        let mut dups: Option<usize> = None;
        let mut p = op[pp].next;
        while p != pp {
            if op[p].pt.y > op[pp].pt.y {
                pp = p;
                dups = None;
            } else if op[p].pt.y == op[pp].pt.y && op[p].pt.x <= op[pp].pt.x {
                if op[p].pt.x < op[pp].pt.x {
                    dups = None;
                    pp = p;
                } else if op[p].next != pp && op[p].prev != pp {
                    dups = Some(p);
                }
            }
            p = op[p].next;
        }
        if let Some(mut d) = dups {
            while d != p {
                if !self.first_is_bottom_pt(p, d) {
                    pp = d;
                }
                d = op[d].next;
                while op[d].pt != op[pp].pt {
                    d = op[d].next;
                }
            }
        }
        pp
    }

    fn get_lowermost_rec(&mut self, out_rec1: usize, out_rec2: usize) -> usize {
        if self.out_recs[out_rec1].bottom_pt.is_none() {
            let b = self.get_bottom_pt(self.out_recs[out_rec1].pts.expect("pts"));
            self.out_recs[out_rec1].bottom_pt = Some(b);
        }
        if self.out_recs[out_rec2].bottom_pt.is_none() {
            let b = self.get_bottom_pt(self.out_recs[out_rec2].pts.expect("pts"));
            self.out_recs[out_rec2].bottom_pt = Some(b);
        }
        let b1 = self.out_recs[out_rec1].bottom_pt.expect("set");
        let b2 = self.out_recs[out_rec2].bottom_pt.expect("set");
        let (p1, p2) = (self.out_pts[b1].pt, self.out_pts[b2].pt);
        if p1.y > p2.y {
            out_rec1
        } else if p1.y < p2.y {
            out_rec2
        } else if p1.x < p2.x {
            out_rec1
        } else if p1.x > p2.x {
            out_rec2
        } else if self.out_pts[b1].next == b1 {
            out_rec2
        } else if self.out_pts[b2].next == b2 {
            out_rec1
        } else if self.first_is_bottom_pt(b1, b2) {
            out_rec1
        } else {
            out_rec2
        }
    }

    fn out_rec1_right_of_out_rec2(&self, mut out_rec1: usize, out_rec2: usize) -> bool {
        loop {
            match self.out_recs[out_rec1].first_left {
                Some(fl) => {
                    out_rec1 = fl;
                    if out_rec1 == out_rec2 {
                        return true;
                    }
                }
                None => return false,
            }
        }
    }

    fn get_out_rec(&self, idx: usize) -> usize {
        let mut outrec = self.poly_outs[idx];
        while outrec != self.poly_outs[self.out_recs[outrec].idx] {
            outrec = self.poly_outs[self.out_recs[outrec].idx];
        }
        outrec
    }

    fn append_polygon(&mut self, e1: usize, e2: usize) {
        let out_rec1 = self.poly_outs[self.e(e1).out_idx as usize];
        let out_rec2 = self.poly_outs[self.e(e2).out_idx as usize];
        let hole_state_rec = if self.out_rec1_right_of_out_rec2(out_rec1, out_rec2) {
            out_rec2
        } else if self.out_rec1_right_of_out_rec2(out_rec2, out_rec1) {
            out_rec1
        } else {
            self.get_lowermost_rec(out_rec1, out_rec2)
        };
        let p1_lft = self.out_recs[out_rec1].pts.expect("pts");
        let p1_rt = self.out_pts[p1_lft].prev;
        let p2_lft = self.out_recs[out_rec2].pts.expect("pts");
        let p2_rt = self.out_pts[p2_lft].prev;
        if self.e(e1).side == EdgeSide::Left {
            if self.e(e2).side == EdgeSide::Left {
                self.reverse_poly_pt_links(Some(p2_lft));
                self.out_pts[p2_lft].next = p1_lft;
                self.out_pts[p1_lft].prev = p2_lft;
                self.out_pts[p1_rt].next = p2_rt;
                self.out_pts[p2_rt].prev = p1_rt;
                self.out_recs[out_rec1].pts = Some(p2_rt);
            } else {
                self.out_pts[p2_rt].next = p1_lft;
                self.out_pts[p1_lft].prev = p2_rt;
                self.out_pts[p2_lft].prev = p1_rt;
                self.out_pts[p1_rt].next = p2_lft;
                self.out_recs[out_rec1].pts = Some(p2_lft);
            }
        } else if self.e(e2).side == EdgeSide::Right {
            self.reverse_poly_pt_links(Some(p2_lft));
            self.out_pts[p1_rt].next = p2_rt;
            self.out_pts[p2_rt].prev = p1_rt;
            self.out_pts[p2_lft].next = p1_lft;
            self.out_pts[p1_lft].prev = p2_lft;
        } else {
            self.out_pts[p1_rt].next = p2_lft;
            self.out_pts[p2_lft].prev = p1_rt;
            self.out_pts[p1_lft].prev = p2_rt;
            self.out_pts[p2_rt].next = p1_lft;
        }
        self.out_recs[out_rec1].bottom_pt = None;
        if hole_state_rec == out_rec2 {
            if self.out_recs[out_rec2].first_left != Some(out_rec1) {
                self.out_recs[out_rec1].first_left = self.out_recs[out_rec2].first_left;
            }
            self.out_recs[out_rec1].is_hole = self.out_recs[out_rec2].is_hole;
        }
        self.out_recs[out_rec2].pts = None;
        self.out_recs[out_rec2].bottom_pt = None;
        self.out_recs[out_rec2].first_left = Some(out_rec1);
        let ok_idx = self.e(e1).out_idx;
        let obsolete_idx = self.e(e2).out_idx;
        self.em(e1).out_idx = -1;
        self.em(e2).out_idx = -1;
        let mut e = self.active_edges;
        while let Some(x) = e {
            if self.e(x).out_idx == obsolete_idx {
                self.em(x).out_idx = ok_idx;
                self.em(x).side = self.e(e1).side;
                break;
            }
            e = self.e(x).next_in_ael;
        }
        self.out_recs[out_rec2].idx = self.out_recs[out_rec1].idx;
    }

    fn reverse_poly_pt_links(&mut self, pp: Option<usize>) {
        let Some(pp) = pp else {
            return;
        };
        let mut pp1 = pp;
        loop {
            let pp2 = self.out_pts[pp1].next;
            self.out_pts[pp1].next = self.out_pts[pp1].prev;
            self.out_pts[pp1].prev = pp2;
            pp1 = pp2;
            if pp1 == pp {
                break;
            }
        }
    }

    fn swap_sides(&mut self, e1: usize, e2: usize) {
        let s = self.e(e1).side;
        self.em(e1).side = self.e(e2).side;
        self.em(e2).side = s;
    }

    fn swap_poly_indexes(&mut self, e1: usize, e2: usize) {
        let o = self.e(e1).out_idx;
        self.em(e1).out_idx = self.e(e2).out_idx;
        self.em(e2).out_idx = o;
    }

    fn fill_wc(fill: PolyFillType, wc: i32) -> i32 {
        match fill {
            PolyFillType::Positive => wc,
            PolyFillType::Negative => -wc,
            _ => wc.abs(),
        }
    }

    fn intersect_edges(&mut self, e1: usize, e2: usize, pt: IntPoint) {
        let e1_contributing = self.e(e1).out_idx >= 0;
        let e2_contributing = self.e(e2).out_idx >= 0;
        // Open paths (a zero WindDelta) are not ported: every edge here closes.
        if self.e(e1).poly_typ == self.e(e2).poly_typ {
            if self.is_even_odd_fill_type(e1) {
                let old = self.e(e1).wind_cnt;
                self.em(e1).wind_cnt = self.e(e2).wind_cnt;
                self.em(e2).wind_cnt = old;
            } else {
                let (e1wc, e2wd) = (self.e(e1).wind_cnt, self.e(e2).wind_delta);
                self.em(e1).wind_cnt = if e1wc + e2wd == 0 { -e1wc } else { e1wc + e2wd };
                let (e2wc, e1wd) = (self.e(e2).wind_cnt, self.e(e1).wind_delta);
                self.em(e2).wind_cnt = if e2wc - e1wd == 0 { -e2wc } else { e2wc - e1wd };
            }
        } else {
            if !self.is_even_odd_fill_type(e2) {
                let d = self.e(e2).wind_delta;
                self.em(e1).wind_cnt2 += d;
            } else {
                let w = self.e(e1).wind_cnt2;
                self.em(e1).wind_cnt2 = if w == 0 { 1 } else { 0 };
            }
            if !self.is_even_odd_fill_type(e1) {
                let d = self.e(e1).wind_delta;
                self.em(e2).wind_cnt2 -= d;
            } else {
                let w = self.e(e2).wind_cnt2;
                self.em(e2).wind_cnt2 = if w == 0 { 1 } else { 0 };
            }
        }
        let fills = |t: PolyType| {
            if t == PolyType::Subject {
                (self.subj_fill_type, self.clip_fill_type)
            } else {
                (self.clip_fill_type, self.subj_fill_type)
            }
        };
        let (e1_fill, e1_fill2) = fills(self.e(e1).poly_typ);
        let (e2_fill, e2_fill2) = fills(self.e(e2).poly_typ);
        let e1_wc = Self::fill_wc(e1_fill, self.e(e1).wind_cnt);
        let e2_wc = Self::fill_wc(e2_fill, self.e(e2).wind_cnt);
        if e1_contributing && e2_contributing {
            if (e1_wc != 0 && e1_wc != 1)
                || (e2_wc != 0 && e2_wc != 1)
                || (self.e(e1).poly_typ != self.e(e2).poly_typ && self.clip_type != ClipType::Xor)
            {
                self.add_local_max_poly(e1, e2, pt);
            } else {
                self.add_out_pt(e1, pt);
                self.add_out_pt(e2, pt);
                self.swap_sides(e1, e2);
                self.swap_poly_indexes(e1, e2);
            }
        } else if e1_contributing {
            if e2_wc == 0 || e2_wc == 1 {
                self.add_out_pt(e1, pt);
                self.swap_sides(e1, e2);
                self.swap_poly_indexes(e1, e2);
            }
        } else if e2_contributing {
            if e1_wc == 0 || e1_wc == 1 {
                self.add_out_pt(e2, pt);
                self.swap_sides(e1, e2);
                self.swap_poly_indexes(e1, e2);
            }
        } else if (e1_wc == 0 || e1_wc == 1) && (e2_wc == 0 || e2_wc == 1) {
            let e1_wc2 = Self::fill_wc(e1_fill2, self.e(e1).wind_cnt2);
            let e2_wc2 = Self::fill_wc(e2_fill2, self.e(e2).wind_cnt2);
            if self.e(e1).poly_typ != self.e(e2).poly_typ {
                self.add_local_min_poly(e1, e2, pt);
            } else if e1_wc == 1 && e2_wc == 1 {
                match self.clip_type {
                    ClipType::Intersection => {
                        if e1_wc2 > 0 && e2_wc2 > 0 {
                            self.add_local_min_poly(e1, e2, pt);
                        }
                    }
                    ClipType::Union => {
                        if e1_wc2 <= 0 && e2_wc2 <= 0 {
                            self.add_local_min_poly(e1, e2, pt);
                        }
                    }
                    ClipType::Difference => {
                        let t = self.e(e1).poly_typ;
                        if (t == PolyType::Clip && e1_wc2 > 0 && e2_wc2 > 0)
                            || (t == PolyType::Subject && e1_wc2 <= 0 && e2_wc2 <= 0)
                        {
                            self.add_local_min_poly(e1, e2, pt);
                        }
                    }
                    ClipType::Xor => {
                        self.add_local_min_poly(e1, e2, pt);
                    }
                }
            } else {
                self.swap_sides(e1, e2);
            }
        }
    }

    fn process_horizontals(&mut self) {
        while let Some(h) = self.pop_edge_from_sel() {
            self.process_horizontal(h);
        }
    }

    fn get_horz_direction(&self, e: usize) -> (Direction, f64, f64) {
        let (b, t) = (self.e(e).bot.x, self.e(e).top.x);
        if b < t {
            (Direction::LeftToRight, b, t)
        } else {
            (Direction::RightToLeft, t, b)
        }
    }

    fn get_next_in_ael(&self, e: usize, dir: Direction) -> Option<usize> {
        if dir == Direction::LeftToRight {
            self.e(e).next_in_ael
        } else {
            self.e(e).prev_in_ael
        }
    }

    fn process_horizontal(&mut self, mut horz_edge: usize) {
        let (mut dir, mut horz_left, mut horz_right) = self.get_horz_direction(horz_edge);
        let is_open = self.e(horz_edge).wind_delta == 0;
        let mut e_last_horz = horz_edge;
        let mut e_max_pair: Option<usize> = None;
        while let Some(n) = self.e(e_last_horz).next_in_lml {
            if self.is_horizontal(n) {
                e_last_horz = n;
            } else {
                break;
            }
        }
        if self.e(e_last_horz).next_in_lml.is_none() {
            e_max_pair = self.get_maxima_pair(e_last_horz);
        }
        let mut curr_max = self.maxima;
        if let Some(mut cm) = curr_max {
            if dir == Direction::LeftToRight {
                let mut c = Some(cm);
                while let Some(x) = c {
                    if self.maxima_pool[x].x <= self.e(horz_edge).bot.x {
                        c = self.maxima_pool[x].next;
                    } else {
                        break;
                    }
                }
                if let Some(x) = c
                    && self.maxima_pool[x].x >= self.e(e_last_horz).top.x
                {
                    c = None;
                }
                curr_max = c;
            } else {
                while let Some(n) = self.maxima_pool[cm].next {
                    if self.maxima_pool[n].x < self.e(horz_edge).bot.x {
                        cm = n;
                    } else {
                        break;
                    }
                }
                curr_max = if self.maxima_pool[cm].x <= self.e(e_last_horz).top.x {
                    None
                } else {
                    Some(cm)
                };
            }
        }
        let mut op1: Option<usize> = None;
        loop {
            let is_last_horz = horz_edge == e_last_horz;
            let mut e = self.get_next_in_ael(horz_edge, dir);
            while let Some(ei) = e {
                if curr_max.is_some() {
                    if dir == Direction::LeftToRight {
                        while let Some(cm) = curr_max {
                            if self.maxima_pool[cm].x < self.e(ei).curr.x {
                                if self.e(horz_edge).out_idx >= 0 && !is_open {
                                    let p = IntPoint::new(
                                        self.maxima_pool[cm].x,
                                        self.e(horz_edge).bot.y,
                                    );
                                    self.add_out_pt(horz_edge, p);
                                }
                                curr_max = self.maxima_pool[cm].next;
                            } else {
                                break;
                            }
                        }
                    } else {
                        while let Some(cm) = curr_max {
                            if self.maxima_pool[cm].x > self.e(ei).curr.x {
                                if self.e(horz_edge).out_idx >= 0 && !is_open {
                                    let p = IntPoint::new(
                                        self.maxima_pool[cm].x,
                                        self.e(horz_edge).bot.y,
                                    );
                                    self.add_out_pt(horz_edge, p);
                                }
                                curr_max = self.maxima_pool[cm].prev;
                            } else {
                                break;
                            }
                        }
                    }
                }
                if (dir == Direction::LeftToRight && self.e(ei).curr.x > horz_right)
                    || (dir == Direction::RightToLeft && self.e(ei).curr.x < horz_left)
                {
                    break;
                }
                if self.e(ei).curr.x == self.e(horz_edge).top.x
                    && let Some(n) = self.e(horz_edge).next_in_lml
                    && self.e(ei).dx < self.e(n).dx
                {
                    break;
                }
                if self.e(horz_edge).out_idx >= 0 && !is_open {
                    let o = self.add_out_pt(horz_edge, self.e(ei).curr);
                    op1 = Some(o);
                    let mut e_next_horz = self.sorted_edges;
                    while let Some(nh) = e_next_horz {
                        if self.e(nh).out_idx >= 0
                            && Self::horz_segments_overlap(
                                self.e(horz_edge).bot.x,
                                self.e(horz_edge).top.x,
                                self.e(nh).bot.x,
                                self.e(nh).top.x,
                            )
                        {
                            let op2 = self.get_last_out_pt(nh);
                            self.add_join(op2, o, self.e(nh).top);
                        }
                        e_next_horz = self.e(nh).next_in_sel;
                    }
                    self.add_ghost_join(o, self.e(horz_edge).bot);
                }
                if Some(ei) == e_max_pair && is_last_horz {
                    if self.e(horz_edge).out_idx >= 0 {
                        let mp = e_max_pair.expect("set");
                        self.add_local_max_poly(horz_edge, mp, self.e(horz_edge).top);
                    }
                    self.delete_from_ael(horz_edge);
                    self.delete_from_ael(e_max_pair.expect("set"));
                    return;
                }
                let pt = IntPoint::new(self.e(ei).curr.x, self.e(horz_edge).curr.y);
                if dir == Direction::LeftToRight {
                    self.intersect_edges(horz_edge, ei, pt);
                } else {
                    self.intersect_edges(ei, horz_edge, pt);
                }
                let e_next = self.get_next_in_ael(ei, dir);
                self.swap_positions_in_ael(horz_edge, ei);
                e = e_next;
            }
            match self.e(horz_edge).next_in_lml {
                Some(n) if self.is_horizontal(n) => {}
                _ => break,
            }
            horz_edge = self.update_edge_into_ael(horz_edge);
            if self.e(horz_edge).out_idx >= 0 {
                self.add_out_pt(horz_edge, self.e(horz_edge).bot);
            }
            (dir, horz_left, horz_right) = self.get_horz_direction(horz_edge);
        }
        if self.e(horz_edge).out_idx >= 0 && op1.is_none() {
            let o = self.get_last_out_pt(horz_edge);
            let mut e_next_horz = self.sorted_edges;
            while let Some(nh) = e_next_horz {
                if self.e(nh).out_idx >= 0
                    && Self::horz_segments_overlap(
                        self.e(horz_edge).bot.x,
                        self.e(horz_edge).top.x,
                        self.e(nh).bot.x,
                        self.e(nh).top.x,
                    )
                {
                    let op2 = self.get_last_out_pt(nh);
                    self.add_join(op2, o, self.e(nh).top);
                }
                e_next_horz = self.e(nh).next_in_sel;
            }
            self.add_ghost_join(o, self.e(horz_edge).top);
        }
        if self.e(horz_edge).next_in_lml.is_some() {
            if self.e(horz_edge).out_idx >= 0 {
                let o = self.add_out_pt(horz_edge, self.e(horz_edge).top);
                horz_edge = self.update_edge_into_ael(horz_edge);
                if self.e(horz_edge).wind_delta == 0 {
                    return;
                }
                let e_prev = self.e(horz_edge).prev_in_ael;
                let e_next = self.e(horz_edge).next_in_ael;
                let hb = self.e(horz_edge).bot;
                if let Some(p) = e_prev
                    && self.e(p).curr.x == hb.x
                    && self.e(p).curr.y == hb.y
                    && self.e(p).wind_delta == 0
                    && (self.e(p).out_idx >= 0
                        && self.e(p).curr.y > self.e(p).top.y
                        && self.slopes_equal3(horz_edge, p))
                {
                    let op2 = self.add_out_pt(p, hb);
                    self.add_join(o, op2, self.e(horz_edge).top);
                } else if let Some(n) = e_next
                    && self.e(n).curr.x == hb.x
                    && self.e(n).curr.y == hb.y
                    && self.e(n).wind_delta != 0
                    && self.e(n).out_idx >= 0
                    && self.e(n).curr.y > self.e(n).top.y
                    && self.slopes_equal3(horz_edge, n)
                {
                    let op2 = self.add_out_pt(n, hb);
                    self.add_join(o, op2, self.e(horz_edge).top);
                }
            } else {
                self.update_edge_into_ael(horz_edge);
            }
        } else {
            if self.e(horz_edge).out_idx >= 0 {
                self.add_out_pt(horz_edge, self.e(horz_edge).top);
            }
            self.delete_from_ael(horz_edge);
        }
    }

    fn get_maxima_pair(&self, e: usize) -> Option<usize> {
        let n = self.e(e).next;
        if self.e(n).top == self.e(e).top && self.e(n).next_in_lml.is_none() {
            return Some(n);
        }
        let p = self.prev(e);
        if self.e(p).top == self.e(e).top && self.e(p).next_in_lml.is_none() {
            return Some(p);
        }
        None
    }

    fn get_maxima_pair_ex(&self, e: usize) -> Option<usize> {
        let result = self.get_maxima_pair(e)?;
        if self.e(result).out_idx == SKIP
            || (self.e(result).next_in_ael == self.e(result).prev_in_ael
                && !self.is_horizontal(result))
        {
            return None;
        }
        Some(result)
    }

    fn process_intersections(&mut self, top_y: f64) -> Result<bool, ClipperError> {
        if self.active_edges.is_none() {
            return Ok(true);
        }
        self.build_intersect_list(top_y);
        if self.intersect_list.is_empty() {
            return Ok(true);
        }
        if self.intersect_list.len() == 1 || self.fixup_intersection_order() {
            self.process_intersect_list();
        } else {
            return Ok(false);
        }
        self.sorted_edges = None;
        Ok(true)
    }

    fn build_intersect_list(&mut self, top_y: f64) {
        let Some(ae) = self.active_edges else {
            return;
        };
        let mut e = Some(ae);
        self.sorted_edges = e;
        while let Some(x) = e {
            let (p, n) = (self.e(x).prev_in_ael, self.e(x).next_in_ael);
            self.em(x).prev_in_sel = p;
            self.em(x).next_in_sel = n;
            self.em(x).curr.x = self.top_x(x, top_y);
            e = n;
        }
        let mut is_modified = true;
        while is_modified && self.sorted_edges.is_some() {
            is_modified = false;
            let mut e = self.sorted_edges.expect("checked");
            while let Some(e_next) = self.e(e).next_in_sel {
                if self.e(e).curr.x > self.e(e_next).curr.x {
                    let mut pt = self.intersect_point(e, e_next);
                    if pt.y < top_y {
                        pt = IntPoint::new(self.top_x(e, top_y), top_y);
                    }
                    self.intersect_list.push(IntersectNode {
                        edge1: e,
                        edge2: e_next,
                        pt,
                    });
                    self.swap_positions_in_sel(e, e_next);
                    is_modified = true;
                } else {
                    e = e_next;
                }
            }
            match self.e(e).prev_in_sel {
                Some(p) => self.em(p).next_in_sel = None,
                None => break,
            }
        }
        self.sorted_edges = None;
    }

    fn edges_adjacent(&self, node: &IntersectNode) -> bool {
        self.e(node.edge1).next_in_sel == Some(node.edge2)
            || self.e(node.edge1).prev_in_sel == Some(node.edge2)
    }

    fn fixup_intersection_order(&mut self) -> bool {
        // `MyIntersectNodeSort.Compare`: descending Y, as a stable sort.
        self.intersect_list.sort_by(|a, b| {
            let i = b.pt.y - a.pt.y;
            if i > 0.0 {
                std::cmp::Ordering::Greater
            } else if i < 0.0 {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        });
        self.copy_ael_to_sel();
        let cnt = self.intersect_list.len();
        for i in 0..cnt {
            if !self.edges_adjacent(&self.intersect_list[i]) {
                let mut j = i + 1;
                while j < cnt && !self.edges_adjacent(&self.intersect_list[j]) {
                    j += 1;
                }
                if j == cnt {
                    return false;
                }
                self.intersect_list.swap(i, j);
            }
            let n = self.intersect_list[i];
            self.swap_positions_in_sel(n.edge1, n.edge2);
        }
        true
    }

    fn process_intersect_list(&mut self) {
        for i in 0..self.intersect_list.len() {
            let n = self.intersect_list[i];
            self.intersect_edges(n.edge1, n.edge2, n.pt);
            self.swap_positions_in_ael(n.edge1, n.edge2);
        }
        self.intersect_list.clear();
    }

    fn top_x(&self, edge: usize, current_y: f64) -> f64 {
        let e = self.e(edge);
        if current_y == e.top.y {
            return e.top.x;
        }
        e.bot.x + round(e.dx * (current_y - e.bot.y))
    }

    fn intersect_point(&self, edge1: usize, edge2: usize) -> IntPoint {
        let (a, b) = (self.e(edge1), self.e(edge2));
        let mut ip = IntPoint::new(0.0, 0.0);
        if a.dx == b.dx {
            ip.y = a.curr.y;
            ip.x = self.top_x(edge1, ip.y);
            return ip;
        }
        if a.delta.x == 0.0 {
            ip.x = a.bot.x;
            if self.is_horizontal(edge2) {
                ip.y = b.bot.y;
            } else {
                let b2 = b.bot.y - (b.bot.x / b.dx);
                ip.y = round(ip.x / b.dx + b2);
            }
        } else if b.delta.x == 0.0 {
            ip.x = b.bot.x;
            if self.is_horizontal(edge1) {
                ip.y = a.bot.y;
            } else {
                let b1 = a.bot.y - (a.bot.x / a.dx);
                ip.y = round(ip.x / a.dx + b1);
            }
        } else {
            let b1 = a.bot.x - a.bot.y * a.dx;
            let b2 = b.bot.x - b.bot.y * b.dx;
            let q = (b2 - b1) / (a.dx - b.dx);
            ip.y = round(q);
            ip.x = if a.dx.abs() < b.dx.abs() {
                round(a.dx * q + b1)
            } else {
                round(b.dx * q + b2)
            };
        }
        if ip.y < a.top.y || ip.y < b.top.y {
            if a.top.y > b.top.y {
                ip.y = a.top.y;
                ip.x = self.top_x(edge2, a.top.y);
                return ip;
            } else {
                ip.y = b.top.y;
            }
            ip.x = if a.dx.abs() < b.dx.abs() {
                self.top_x(edge1, ip.y)
            } else {
                self.top_x(edge2, ip.y)
            };
        }
        if ip.y > a.curr.y {
            ip.y = a.curr.y;
            ip.x = if a.dx.abs() > b.dx.abs() {
                self.top_x(edge2, ip.y)
            } else {
                self.top_x(edge1, ip.y)
            };
        }
        ip
    }

    fn is_maxima(&self, e: usize, y: f64) -> bool {
        self.e(e).top.y == y && self.e(e).next_in_lml.is_none()
    }

    fn is_intermediate(&self, e: usize, y: f64) -> bool {
        self.e(e).top.y == y && self.e(e).next_in_lml.is_some()
    }

    fn process_edges_at_top_of_scanbeam(&mut self, top_y: f64) -> Result<(), ClipperError> {
        let mut e = self.active_edges;
        while let Some(ei) = e {
            let mut is_maxima_edge = self.is_maxima(ei, top_y);
            if is_maxima_edge {
                let e_max_pair = self.get_maxima_pair_ex(ei);
                is_maxima_edge = e_max_pair.is_none_or(|p| !self.is_horizontal(p));
            }
            if is_maxima_edge {
                if self.strictly_simple {
                    self.insert_maxima(self.e(ei).top.x);
                }
                let e_prev = self.e(ei).prev_in_ael;
                self.do_maxima(ei)?;
                e = match e_prev {
                    None => self.active_edges,
                    Some(p) => self.e(p).next_in_ael,
                };
            } else {
                let mut ei = ei;
                if self.is_intermediate(ei, top_y)
                    && self.is_horizontal(self.e(ei).next_in_lml.expect("intermediate"))
                {
                    ei = self.update_edge_into_ael(ei);
                    if self.e(ei).out_idx >= 0 {
                        self.add_out_pt(ei, self.e(ei).bot);
                    }
                    self.add_edge_to_sel(ei);
                } else {
                    let x = self.top_x(ei, top_y);
                    self.em(ei).curr.x = x;
                    self.em(ei).curr.y = top_y;
                }
                if self.strictly_simple {
                    let e_prev = self.e(ei).prev_in_ael;
                    if let Some(p) = e_prev
                        && self.e(ei).out_idx >= 0
                        && self.e(ei).wind_delta != 0
                        && self.e(p).out_idx >= 0
                        && self.e(p).curr.x == self.e(ei).curr.x
                        && self.e(p).wind_delta != 0
                    {
                        let ip = self.e(ei).curr;
                        let op = self.add_out_pt(p, ip);
                        let op2 = self.add_out_pt(ei, ip);
                        self.add_join(op, op2, ip);
                    }
                }
                e = self.e(ei).next_in_ael;
            }
        }
        self.process_horizontals();
        self.maxima = None;
        let mut e = self.active_edges;
        while let Some(mut ei) = e {
            if self.is_intermediate(ei, top_y) {
                let mut op: Option<usize> = None;
                if self.e(ei).out_idx >= 0 {
                    op = Some(self.add_out_pt(ei, self.e(ei).top));
                }
                ei = self.update_edge_into_ael(ei);
                let e_prev = self.e(ei).prev_in_ael;
                let e_next = self.e(ei).next_in_ael;
                let bot = self.e(ei).bot;
                if let Some(p) = e_prev
                    && self.e(p).curr.x == bot.x
                    && self.e(p).curr.y == bot.y
                    && op.is_some()
                    && self.e(p).out_idx >= 0
                    && self.e(p).curr.y == self.e(p).top.y
                    && slopes_equal5(
                        self.e(ei).curr,
                        self.e(ei).top,
                        self.e(p).curr,
                        self.e(p).top,
                        self.use_full_range,
                    )
                    && self.e(ei).wind_delta != 0
                    && self.e(p).wind_delta != 0
                {
                    // The JavaScript port reads an undeclared `ePrev2` here.
                    return Err(ClipperError::EPrev2);
                } else if let Some(n) = e_next
                    && self.e(n).curr.x == bot.x
                    && self.e(n).curr.y == bot.y
                    && op.is_some()
                    && self.e(n).out_idx >= 0
                    && self.e(n).curr.y == self.e(n).top.y
                    && slopes_equal5(
                        self.e(ei).curr,
                        self.e(ei).top,
                        self.e(n).curr,
                        self.e(n).top,
                        self.use_full_range,
                    )
                    && self.e(ei).wind_delta != 0
                    && self.e(n).wind_delta != 0
                {
                    let op2 = self.add_out_pt(n, bot);
                    self.add_join(op.expect("checked"), op2, self.e(ei).top);
                }
            }
            e = self.e(ei).next_in_ael;
        }
        Ok(())
    }

    fn do_maxima(&mut self, e: usize) -> Result<(), ClipperError> {
        let Some(e_max_pair) = self.get_maxima_pair_ex(e) else {
            if self.e(e).out_idx >= 0 {
                self.add_out_pt(e, self.e(e).top);
            }
            self.delete_from_ael(e);
            return Ok(());
        };
        let mut e_next = self.e(e).next_in_ael;
        while let Some(n) = e_next {
            if n == e_max_pair {
                break;
            }
            let top = self.e(e).top;
            self.intersect_edges(e, n, top);
            self.swap_positions_in_ael(e, n);
            e_next = self.e(e).next_in_ael;
        }
        if self.e(e).out_idx == -1 && self.e(e_max_pair).out_idx == -1 {
            self.delete_from_ael(e);
            self.delete_from_ael(e_max_pair);
        } else if self.e(e).out_idx >= 0 && self.e(e_max_pair).out_idx >= 0 {
            self.add_local_max_poly(e, e_max_pair, self.e(e).top);
            self.delete_from_ael(e);
            self.delete_from_ael(e_max_pair);
        } else {
            return Err(ClipperError::Message("DoMaxima error"));
        }
        Ok(())
    }

    fn point_count(&self, pts: Option<usize>) -> usize {
        let Some(start) = pts else {
            return 0;
        };
        let mut result = 0;
        let mut p = start;
        loop {
            result += 1;
            p = self.out_pts[p].next;
            if p == start {
                break;
            }
        }
        result
    }

    fn build_result(&self, polyg: &mut Paths) {
        for &out_rec in &self.poly_outs {
            let Some(pts) = self.out_recs[out_rec].pts else {
                continue;
            };
            let mut p = self.out_pts[pts].prev;
            let cnt = self.point_count(Some(p));
            if cnt < 2 {
                continue;
            }
            let mut pg = Vec::with_capacity(cnt);
            for _ in 0..cnt {
                pg.push(self.out_pts[p].pt);
                p = self.out_pts[p].prev;
            }
            polyg.push(pg);
        }
    }

    fn build_result2(&mut self, polytree: &mut PolyTree) {
        polytree.clear();
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            let cnt = self.point_count(self.out_recs[out_rec].pts);
            if (self.out_recs[out_rec].is_open && cnt < 2)
                || (!self.out_recs[out_rec].is_open && cnt < 3)
            {
                continue;
            }
            self.fix_hole_linkage(out_rec);
            let pn = polytree.nodes.len();
            let mut polygon = Vec::with_capacity(cnt);
            let mut op = self.out_pts[self.out_recs[out_rec].pts.expect("counted")].prev;
            for _ in 0..cnt {
                polygon.push(self.out_pts[op].pt);
                op = self.out_pts[op].prev;
            }
            polytree.nodes.push(PolyNode {
                polygon,
                ..PolyNode::default()
            });
            self.out_recs[out_rec].poly_node = Some(pn);
        }
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            let Some(pn) = self.out_recs[out_rec].poly_node else {
                continue;
            };
            if self.out_recs[out_rec].is_open {
                polytree.nodes[pn].is_open = true;
                polytree.add_child(0, pn);
            } else if let Some(fl) = self.out_recs[out_rec].first_left
                && let Some(fpn) = self.out_recs[fl].poly_node
            {
                polytree.add_child(fpn, pn);
            } else {
                polytree.add_child(0, pn);
            }
        }
    }

    fn fixup_out_polygon(&mut self, out_rec: usize) {
        let mut last_ok: Option<usize> = None;
        self.out_recs[out_rec].bottom_pt = None;
        let mut pp = self.out_recs[out_rec].pts.expect("pts");
        let preserve_col = self.preserve_collinear || self.strictly_simple;
        loop {
            let (prev, next) = (self.out_pts[pp].prev, self.out_pts[pp].next);
            if prev == pp || prev == next {
                self.out_recs[out_rec].pts = None;
                return;
            }
            let (pp_pt, prev_pt, next_pt) = (
                self.out_pts[pp].pt,
                self.out_pts[prev].pt,
                self.out_pts[next].pt,
            );
            if pp_pt == next_pt
                || pp_pt == prev_pt
                || (slopes_equal4(prev_pt, pp_pt, next_pt, self.use_full_range)
                    && (!preserve_col || !pt2_is_between_pt1_and_pt3(prev_pt, pp_pt, next_pt)))
            {
                last_ok = None;
                self.out_pts[prev].next = next;
                self.out_pts[next].prev = prev;
                pp = prev;
            } else if Some(pp) == last_ok {
                break;
            } else {
                if last_ok.is_none() {
                    last_ok = Some(pp);
                }
                pp = next;
            }
        }
        self.out_recs[out_rec].pts = Some(pp);
    }

    fn dup_out_pt(&mut self, out_pt: usize, insert_after: bool) -> usize {
        let (pt, idx) = (self.out_pts[out_pt].pt, self.out_pts[out_pt].idx);
        let result = self.new_out_pt(idx, pt);
        if insert_after {
            let n = self.out_pts[out_pt].next;
            self.out_pts[result].next = n;
            self.out_pts[result].prev = out_pt;
            self.out_pts[n].prev = result;
            self.out_pts[out_pt].next = result;
        } else {
            let p = self.out_pts[out_pt].prev;
            self.out_pts[result].prev = p;
            self.out_pts[result].next = out_pt;
            self.out_pts[p].next = result;
            self.out_pts[out_pt].prev = result;
        }
        result
    }

    fn get_overlap(a1: f64, a2: f64, b1: f64, b2: f64) -> (bool, f64, f64) {
        let (left, right) = if a1 < a2 {
            if b1 < b2 {
                (a1.max(b1), a2.min(b2))
            } else {
                (a1.max(b2), a2.min(b1))
            }
        } else if b1 < b2 {
            (a2.max(b1), a1.min(b2))
        } else {
            (a2.max(b2), a1.min(b1))
        };
        (left < right, left, right)
    }

    #[allow(clippy::too_many_arguments)]
    fn join_horz(
        &mut self,
        mut op1: usize,
        mut op1b: usize,
        mut op2: usize,
        mut op2b: usize,
        pt: IntPoint,
        discard_left: bool,
    ) -> bool {
        let dir1 = if self.out_pts[op1].pt.x > self.out_pts[op1b].pt.x {
            Direction::RightToLeft
        } else {
            Direction::LeftToRight
        };
        let dir2 = if self.out_pts[op2].pt.x > self.out_pts[op2b].pt.x {
            Direction::RightToLeft
        } else {
            Direction::LeftToRight
        };
        if dir1 == dir2 {
            return false;
        }
        let walk = |this: &mut Self, op: &mut usize, opb: &mut usize, dir: Direction| {
            if dir == Direction::LeftToRight {
                loop {
                    let n = this.out_pts[*op].next;
                    let (np, p) = (this.out_pts[n].pt, this.out_pts[*op].pt);
                    if np.x <= pt.x && np.x >= p.x && np.y == pt.y {
                        *op = n;
                    } else {
                        break;
                    }
                }
                if discard_left && this.out_pts[*op].pt.x != pt.x {
                    *op = this.out_pts[*op].next;
                }
                *opb = this.dup_out_pt(*op, !discard_left);
                if this.out_pts[*opb].pt != pt {
                    *op = *opb;
                    this.out_pts[*op].pt = pt;
                    *opb = this.dup_out_pt(*op, !discard_left);
                }
            } else {
                loop {
                    let n = this.out_pts[*op].next;
                    let (np, p) = (this.out_pts[n].pt, this.out_pts[*op].pt);
                    if np.x >= pt.x && np.x <= p.x && np.y == pt.y {
                        *op = n;
                    } else {
                        break;
                    }
                }
                if !discard_left && this.out_pts[*op].pt.x != pt.x {
                    *op = this.out_pts[*op].next;
                }
                *opb = this.dup_out_pt(*op, discard_left);
                if this.out_pts[*opb].pt != pt {
                    *op = *opb;
                    this.out_pts[*op].pt = pt;
                    *opb = this.dup_out_pt(*op, discard_left);
                }
            }
        };
        walk(self, &mut op1, &mut op1b, dir1);
        walk(self, &mut op2, &mut op2b, dir2);
        if (dir1 == Direction::LeftToRight) == discard_left {
            self.out_pts[op1].prev = op2;
            self.out_pts[op2].next = op1;
            self.out_pts[op1b].next = op2b;
            self.out_pts[op2b].prev = op1b;
        } else {
            self.out_pts[op1].next = op2;
            self.out_pts[op2].prev = op1;
            self.out_pts[op1b].prev = op2b;
            self.out_pts[op2b].next = op1b;
        }
        true
    }

    fn join_points(&mut self, j: &mut Join, out_rec1: usize, out_rec2: usize) -> bool {
        let mut op1 = j.out_pt1;
        let mut op2 = j.out_pt2;
        let is_horizontal = self.out_pts[j.out_pt1].pt.y == j.off_pt.y;
        if is_horizontal
            && j.off_pt == self.out_pts[j.out_pt1].pt
            && j.off_pt == self.out_pts[j.out_pt2].pt
        {
            if out_rec1 != out_rec2 {
                return false;
            }
            let mut op1b = self.out_pts[j.out_pt1].next;
            while op1b != op1 && self.out_pts[op1b].pt == j.off_pt {
                op1b = self.out_pts[op1b].next;
            }
            let reverse1 = self.out_pts[op1b].pt.y > j.off_pt.y;
            let mut op2b = self.out_pts[j.out_pt2].next;
            while op2b != op2 && self.out_pts[op2b].pt == j.off_pt {
                op2b = self.out_pts[op2b].next;
            }
            let reverse2 = self.out_pts[op2b].pt.y > j.off_pt.y;
            if reverse1 == reverse2 {
                return false;
            }
            if reverse1 {
                let op1b = self.dup_out_pt(op1, false);
                let op2b = self.dup_out_pt(op2, true);
                self.out_pts[op1].prev = op2;
                self.out_pts[op2].next = op1;
                self.out_pts[op1b].next = op2b;
                self.out_pts[op2b].prev = op1b;
                j.out_pt1 = op1;
                j.out_pt2 = op1b;
            } else {
                let op1b = self.dup_out_pt(op1, true);
                let op2b = self.dup_out_pt(op2, false);
                self.out_pts[op1].next = op2;
                self.out_pts[op2].prev = op1;
                self.out_pts[op1b].prev = op2b;
                self.out_pts[op2b].next = op1b;
                j.out_pt1 = op1;
                j.out_pt2 = op1b;
            }
            true
        } else if is_horizontal {
            let mut op1b = op1;
            let o = &self.out_pts;
            while o[o[op1].prev].pt.y == o[op1].pt.y && o[op1].prev != op1b && o[op1].prev != op2 {
                op1 = o[op1].prev;
            }
            while o[o[op1b].next].pt.y == o[op1b].pt.y && o[op1b].next != op1 && o[op1b].next != op2
            {
                op1b = o[op1b].next;
            }
            if o[op1b].next == op1 || o[op1b].next == op2 {
                return false;
            }
            let mut op2b = op2;
            while o[o[op2].prev].pt.y == o[op2].pt.y && o[op2].prev != op2b && o[op2].prev != op1b {
                op2 = o[op2].prev;
            }
            while o[o[op2b].next].pt.y == o[op2b].pt.y && o[op2b].next != op2 && o[op2b].next != op1
            {
                op2b = o[op2b].next;
            }
            if o[op2b].next == op2 || o[op2b].next == op1 {
                return false;
            }
            let (overlap, left, right) =
                Self::get_overlap(o[op1].pt.x, o[op1b].pt.x, o[op2].pt.x, o[op2b].pt.x);
            if !overlap {
                return false;
            }
            let (pt, discard_left_side) = if o[op1].pt.x >= left && o[op1].pt.x <= right {
                (o[op1].pt, o[op1].pt.x > o[op1b].pt.x)
            } else if o[op2].pt.x >= left && o[op2].pt.x <= right {
                (o[op2].pt, o[op2].pt.x > o[op2b].pt.x)
            } else if o[op1b].pt.x >= left && o[op1b].pt.x <= right {
                (o[op1b].pt, o[op1b].pt.x > o[op1].pt.x)
            } else {
                (o[op2b].pt, o[op2b].pt.x > o[op2].pt.x)
            };
            j.out_pt1 = op1;
            j.out_pt2 = op2;
            self.join_horz(op1, op1b, op2, op2b, pt, discard_left_side)
        } else {
            let o = &self.out_pts;
            let mut op1b = o[op1].next;
            while o[op1b].pt == o[op1].pt && op1b != op1 {
                op1b = o[op1b].next;
            }
            let reverse1 = o[op1b].pt.y > o[op1].pt.y
                || !slopes_equal4(o[op1].pt, o[op1b].pt, j.off_pt, self.use_full_range);
            if reverse1 {
                op1b = o[op1].prev;
                while o[op1b].pt == o[op1].pt && op1b != op1 {
                    op1b = o[op1b].prev;
                }
                if o[op1b].pt.y > o[op1].pt.y
                    || !slopes_equal4(o[op1].pt, o[op1b].pt, j.off_pt, self.use_full_range)
                {
                    return false;
                }
            }
            let mut op2b = o[op2].next;
            while o[op2b].pt == o[op2].pt && op2b != op2 {
                op2b = o[op2b].next;
            }
            let reverse2 = o[op2b].pt.y > o[op2].pt.y
                || !slopes_equal4(o[op2].pt, o[op2b].pt, j.off_pt, self.use_full_range);
            if reverse2 {
                op2b = o[op2].prev;
                while o[op2b].pt == o[op2].pt && op2b != op2 {
                    op2b = o[op2b].prev;
                }
                if o[op2b].pt.y > o[op2].pt.y
                    || !slopes_equal4(o[op2].pt, o[op2b].pt, j.off_pt, self.use_full_range)
                {
                    return false;
                }
            }
            if op1b == op1
                || op2b == op2
                || op1b == op2b
                || (out_rec1 == out_rec2 && reverse1 == reverse2)
            {
                return false;
            }
            if reverse1 {
                let op1b = self.dup_out_pt(op1, false);
                let op2b = self.dup_out_pt(op2, true);
                self.out_pts[op1].prev = op2;
                self.out_pts[op2].next = op1;
                self.out_pts[op1b].next = op2b;
                self.out_pts[op2b].prev = op1b;
                j.out_pt1 = op1;
                j.out_pt2 = op1b;
            } else {
                let op1b = self.dup_out_pt(op1, true);
                let op2b = self.dup_out_pt(op2, false);
                self.out_pts[op1].next = op2;
                self.out_pts[op2].prev = op1;
                self.out_pts[op1b].prev = op2b;
                self.out_pts[op2b].next = op1b;
                j.out_pt1 = op1;
                j.out_pt2 = op1b;
            }
            true
        }
    }

    fn point_in_polygon_op(&self, pt: IntPoint, mut op: usize) -> i32 {
        let mut result = 0;
        let start_op = op;
        let (ptx, pty) = (pt.x, pt.y);
        let (mut poly0x, mut poly0y) = (self.out_pts[op].pt.x, self.out_pts[op].pt.y);
        loop {
            op = self.out_pts[op].next;
            let (poly1x, poly1y) = (self.out_pts[op].pt.x, self.out_pts[op].pt.y);
            if poly1y == pty
                && (poly1x == ptx || (poly0y == pty && ((poly1x > ptx) == (poly0x < ptx))))
            {
                return -1;
            }
            if (poly0y < pty) != (poly1y < pty) {
                if poly0x >= ptx {
                    if poly1x > ptx {
                        result = 1 - result;
                    } else {
                        let d = (poly0x - ptx) * (poly1y - pty) - (poly1x - ptx) * (poly0y - pty);
                        if d == 0.0 {
                            return -1;
                        }
                        if (d > 0.0) == (poly1y > poly0y) {
                            result = 1 - result;
                        }
                    }
                } else if poly1x > ptx {
                    let d = (poly0x - ptx) * (poly1y - pty) - (poly1x - ptx) * (poly0y - pty);
                    if d == 0.0 {
                        return -1;
                    }
                    if (d > 0.0) == (poly1y > poly0y) {
                        result = 1 - result;
                    }
                }
            }
            poly0x = poly1x;
            poly0y = poly1y;
            if start_op == op {
                break;
            }
        }
        result
    }

    fn poly2_contains_poly1(&self, out_pt1: usize, out_pt2: usize) -> bool {
        let mut op = out_pt1;
        loop {
            let res = self.point_in_polygon_op(self.out_pts[op].pt, out_pt2);
            if res >= 0 {
                return res > 0;
            }
            op = self.out_pts[op].next;
            if op == out_pt1 {
                break;
            }
        }
        true
    }

    fn parse_first_left(&self, mut first_left: Option<usize>) -> Option<usize> {
        while let Some(f) = first_left {
            if self.out_recs[f].pts.is_none() {
                first_left = self.out_recs[f].first_left;
            } else {
                break;
            }
        }
        first_left
    }

    fn fixup_first_lefts1(&mut self, old_out_rec: usize, new_out_rec: usize) {
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            let first_left = self.parse_first_left(self.out_recs[out_rec].first_left);
            if let Some(pts) = self.out_recs[out_rec].pts
                && first_left == Some(old_out_rec)
                && self.poly2_contains_poly1(pts, self.out_recs[new_out_rec].pts.expect("pts"))
            {
                self.out_recs[out_rec].first_left = Some(new_out_rec);
            }
        }
    }

    fn fixup_first_lefts2(&mut self, inner_out_rec: usize, outer_out_rec: usize) {
        let orfl = self.out_recs[outer_out_rec].first_left;
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            let Some(pts) = self.out_recs[out_rec].pts else {
                continue;
            };
            if out_rec == outer_out_rec || out_rec == inner_out_rec {
                continue;
            }
            let first_left = self.parse_first_left(self.out_recs[out_rec].first_left);
            if first_left != orfl
                && first_left != Some(inner_out_rec)
                && first_left != Some(outer_out_rec)
            {
                continue;
            }
            if self.poly2_contains_poly1(pts, self.out_recs[inner_out_rec].pts.expect("pts")) {
                self.out_recs[out_rec].first_left = Some(inner_out_rec);
            } else if self.poly2_contains_poly1(pts, self.out_recs[outer_out_rec].pts.expect("pts"))
            {
                self.out_recs[out_rec].first_left = Some(outer_out_rec);
            } else if self.out_recs[out_rec].first_left == Some(inner_out_rec)
                || self.out_recs[out_rec].first_left == Some(outer_out_rec)
            {
                self.out_recs[out_rec].first_left = orfl;
            }
        }
    }

    fn fixup_first_lefts3(&mut self, old_out_rec: usize, new_out_rec: usize) {
        for i in 0..self.poly_outs.len() {
            let out_rec = self.poly_outs[i];
            let first_left = self.parse_first_left(self.out_recs[out_rec].first_left);
            if self.out_recs[out_rec].pts.is_some() && first_left == Some(old_out_rec) {
                self.out_recs[out_rec].first_left = Some(new_out_rec);
            }
        }
    }

    fn join_common_edges(&mut self) {
        for i in 0..self.joins.len() {
            let mut join = self.joins[i];
            let out_rec1 = self.get_out_rec(self.out_pts[join.out_pt1].idx);
            let mut out_rec2 = self.get_out_rec(self.out_pts[join.out_pt2].idx);
            if self.out_recs[out_rec1].pts.is_none() || self.out_recs[out_rec2].pts.is_none() {
                continue;
            }
            if self.out_recs[out_rec1].is_open || self.out_recs[out_rec2].is_open {
                continue;
            }
            let hole_state_rec = if out_rec1 == out_rec2 {
                out_rec1
            } else if self.out_rec1_right_of_out_rec2(out_rec1, out_rec2) {
                out_rec2
            } else if self.out_rec1_right_of_out_rec2(out_rec2, out_rec1) {
                out_rec1
            } else {
                self.get_lowermost_rec(out_rec1, out_rec2)
            };
            let joined = self.join_points(&mut join, out_rec1, out_rec2);
            self.joins[i] = join;
            if !joined {
                continue;
            }
            if out_rec1 == out_rec2 {
                self.out_recs[out_rec1].pts = Some(join.out_pt1);
                self.out_recs[out_rec1].bottom_pt = None;
                out_rec2 = self.create_out_rec();
                self.out_recs[out_rec2].pts = Some(join.out_pt2);
                self.update_out_pt_idxs(out_rec2);
                let (p1, p2) = (
                    self.out_recs[out_rec1].pts.expect("set"),
                    self.out_recs[out_rec2].pts.expect("set"),
                );
                if self.poly2_contains_poly1(p2, p1) {
                    self.out_recs[out_rec2].is_hole = !self.out_recs[out_rec1].is_hole;
                    self.out_recs[out_rec2].first_left = Some(out_rec1);
                    if self.using_poly_tree {
                        self.fixup_first_lefts2(out_rec2, out_rec1);
                    }
                    if (self.out_recs[out_rec2].is_hole ^ self.reverse_solution)
                        == (self.area_out_rec(out_rec2) > 0.0)
                    {
                        self.reverse_poly_pt_links(self.out_recs[out_rec2].pts);
                    }
                } else if self.poly2_contains_poly1(p1, p2) {
                    self.out_recs[out_rec2].is_hole = self.out_recs[out_rec1].is_hole;
                    self.out_recs[out_rec1].is_hole = !self.out_recs[out_rec2].is_hole;
                    self.out_recs[out_rec2].first_left = self.out_recs[out_rec1].first_left;
                    self.out_recs[out_rec1].first_left = Some(out_rec2);
                    if self.using_poly_tree {
                        self.fixup_first_lefts2(out_rec1, out_rec2);
                    }
                    if (self.out_recs[out_rec1].is_hole ^ self.reverse_solution)
                        == (self.area_out_rec(out_rec1) > 0.0)
                    {
                        self.reverse_poly_pt_links(self.out_recs[out_rec1].pts);
                    }
                } else {
                    self.out_recs[out_rec2].is_hole = self.out_recs[out_rec1].is_hole;
                    self.out_recs[out_rec2].first_left = self.out_recs[out_rec1].first_left;
                    if self.using_poly_tree {
                        self.fixup_first_lefts1(out_rec1, out_rec2);
                    }
                }
            } else {
                self.out_recs[out_rec2].pts = None;
                self.out_recs[out_rec2].bottom_pt = None;
                self.out_recs[out_rec2].idx = self.out_recs[out_rec1].idx;
                self.out_recs[out_rec1].is_hole = self.out_recs[hole_state_rec].is_hole;
                if hole_state_rec == out_rec2 {
                    self.out_recs[out_rec1].first_left = self.out_recs[out_rec2].first_left;
                }
                self.out_recs[out_rec2].first_left = Some(out_rec1);
                if self.using_poly_tree {
                    self.fixup_first_lefts3(out_rec2, out_rec1);
                }
            }
        }
    }

    fn update_out_pt_idxs(&mut self, outrec: usize) {
        let start = self.out_recs[outrec].pts.expect("pts");
        let idx = self.out_recs[outrec].idx;
        let mut op = start;
        loop {
            self.out_pts[op].idx = idx;
            op = self.out_pts[op].prev;
            if op == start {
                break;
            }
        }
    }

    fn do_simple_polygons(&mut self) {
        let mut i = 0;
        while i < self.poly_outs.len() {
            let outrec = self.poly_outs[i];
            i += 1;
            let Some(mut op) = self.out_recs[outrec].pts else {
                continue;
            };
            if self.out_recs[outrec].is_open {
                continue;
            }
            loop {
                let mut op2 = self.out_pts[op].next;
                while Some(op2) != self.out_recs[outrec].pts {
                    if self.out_pts[op].pt == self.out_pts[op2].pt
                        && self.out_pts[op2].next != op
                        && self.out_pts[op2].prev != op
                    {
                        let op3 = self.out_pts[op].prev;
                        let op4 = self.out_pts[op2].prev;
                        self.out_pts[op].prev = op4;
                        self.out_pts[op4].next = op;
                        self.out_pts[op2].prev = op3;
                        self.out_pts[op3].next = op2;
                        self.out_recs[outrec].pts = Some(op);
                        let outrec2 = self.create_out_rec();
                        self.out_recs[outrec2].pts = Some(op2);
                        self.update_out_pt_idxs(outrec2);
                        if self.poly2_contains_poly1(op2, op) {
                            self.out_recs[outrec2].is_hole = !self.out_recs[outrec].is_hole;
                            self.out_recs[outrec2].first_left = Some(outrec);
                            if self.using_poly_tree {
                                self.fixup_first_lefts2(outrec2, outrec);
                            }
                        } else if self.poly2_contains_poly1(op, op2) {
                            self.out_recs[outrec2].is_hole = self.out_recs[outrec].is_hole;
                            self.out_recs[outrec].is_hole = !self.out_recs[outrec2].is_hole;
                            self.out_recs[outrec2].first_left = self.out_recs[outrec].first_left;
                            self.out_recs[outrec].first_left = Some(outrec2);
                            if self.using_poly_tree {
                                self.fixup_first_lefts2(outrec, outrec2);
                            }
                        } else {
                            self.out_recs[outrec2].is_hole = self.out_recs[outrec].is_hole;
                            self.out_recs[outrec2].first_left = self.out_recs[outrec].first_left;
                            if self.using_poly_tree {
                                self.fixup_first_lefts1(outrec, outrec2);
                            }
                        }
                        op2 = op;
                    }
                    op2 = self.out_pts[op2].next;
                }
                op = self.out_pts[op].next;
                if Some(op) == self.out_recs[outrec].pts {
                    break;
                }
            }
        }
    }

    fn area_op(&self, op: Option<usize>) -> f64 {
        let Some(first) = op else {
            return 0.0;
        };
        let mut a = 0.0;
        let mut op = first;
        loop {
            let prev = self.out_pts[op].prev;
            a += (self.out_pts[prev].pt.x + self.out_pts[op].pt.x)
                * (self.out_pts[prev].pt.y - self.out_pts[op].pt.y);
            op = self.out_pts[op].next;
            if op == first {
                break;
            }
        }
        a * 0.5
    }

    fn area_out_rec(&self, out_rec: usize) -> f64 {
        self.area_op(self.out_recs[out_rec].pts)
    }
}

/// `ClipperLib.Clipper.GetBounds`: (left, top, right, bottom).
pub(crate) fn get_bounds(paths: &[Path]) -> (f64, f64, f64, f64) {
    let mut i = 0;
    let cnt = paths.len();
    while i < cnt && paths[i].is_empty() {
        i += 1;
    }
    if i == cnt {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let (mut left, mut top) = (paths[i][0].x, paths[i][0].y);
    let (mut right, mut bottom) = (left, top);
    for path in &paths[i..] {
        for p in path {
            if p.x < left {
                left = p.x;
            } else if p.x > right {
                right = p.x;
            }
            if p.y < top {
                top = p.y;
            } else if p.y > bottom {
                bottom = p.y;
            }
        }
    }
    (left, top, right, bottom)
}
