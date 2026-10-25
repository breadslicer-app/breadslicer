// Parity oracle for block 9a of the native port: clipper-lib 6.4.2.2 (the
// package DragonFruit's raft code uses) driven as `polygonSet2d.ts` drives it:
// PolyTree union and difference with NonZero fill (StrictlySimple on and off)
// and ClipperOffset with miter joins on closed polygons (miter limit 2, arc
// tolerance 2), both offset signs. Inputs are integer paths built to reach the
// engine's hard cases: discs and boxes, rings with holes, grid rectangles
// sharing edges (horizontal joins), duplicate and collinear points,
// self-intersecting stars, coincident copies and coordinates past loRange.
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_clipper.json
import ClipperLib from 'clipper-lib';

let seed = 0xc11b;
function rand(): number {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const range = (lo: number, hi: number) => lo + (hi - lo) * rand();
const pick = <T>(xs: readonly T[]): T => xs[Math.floor(rand() * xs.length)];
type P = { X: number; Y: number };

function disc(cx: number, cy: number, r: number, n: number, ccw = true): P[] {
    const out: P[] = [];
    for (let i = 0; i < n; i++) {
        const a = (ccw ? 1 : -1) * (i / n) * Math.PI * 2;
        out.push({ X: Math.round(cx + r * Math.cos(a)), Y: Math.round(cy + r * Math.sin(a)) });
    }
    return out;
}
function box(x0: number, y0: number, x1: number, y1: number, ccw = true): P[] {
    const b = [{ X: x0, Y: y0 }, { X: x1, Y: y0 }, { X: x1, Y: y1 }, { X: x0, Y: y1 }];
    return ccw ? b : b.reverse();
}
function star(cx: number, cy: number, r: number, n: number): P[] {
    const out: P[] = [];
    for (let i = 0; i < n; i++) {
        const a = (i * 2 * Math.PI * (n >> 1)) / n;
        out.push({ X: Math.round(cx + r * Math.cos(a)), Y: Math.round(cy + r * Math.sin(a)) });
    }
    return out;
}
function randomShape(scale: number): P[][] {
    const cx = range(-20, 20) * scale, cy = range(-20, 20) * scale;
    switch (pick(['disc', 'box', 'ring', 'grid', 'dups', 'star', 'poly', 'copy', 'stairs', 'comb', 'stairs', 'comb'] as const)) {
        case 'disc': return [disc(cx, cy, range(1, 15) * scale, Math.floor(range(3, 40)))];
        case 'box': {
            const w = Math.round(range(1, 20) * scale), h = Math.round(range(1, 20) * scale);
            return [box(Math.round(cx), Math.round(cy), Math.round(cx) + w, Math.round(cy) + h)];
        }
        case 'ring': {
            const r = range(4, 15) * scale;
            return [disc(cx, cy, r, Math.floor(range(6, 30))), disc(cx, cy, r * range(0.3, 0.8), Math.floor(range(4, 24)), false)];
        }
        case 'grid': {
            const g = Math.round(range(1, 6) * scale);
            const out: P[][] = [];
            const ox = Math.round(cx / g) * g, oy = Math.round(cy / g) * g;
            for (let i = 0; i < Math.floor(range(2, 7)); i++) {
                const gx = ox + Math.floor(range(0, 4)) * g, gy = oy + Math.floor(range(0, 4)) * g;
                out.push(box(gx, gy, gx + g * Math.floor(range(1, 3)), gy + g * Math.floor(range(1, 3))));
            }
            return out;
        }
        case 'dups': {
            const d = disc(cx, cy, range(2, 12) * scale, Math.floor(range(4, 16)));
            const out: P[] = [];
            for (const p of d) {
                out.push(p);
                if (rand() < 0.3) out.push({ ...p });
                if (rand() < 0.2) out.push({ X: p.X + 1, Y: p.Y });
            }
            // A collinear midpoint on one edge.
            const a = out[0], b = out[1];
            if ((a.X + b.X) % 2 === 0 && (a.Y + b.Y) % 2 === 0) out.splice(1, 0, { X: (a.X + b.X) / 2, Y: (a.Y + b.Y) / 2 });
            return [out];
        }
        case 'star': return [star(cx, cy, range(3, 15) * scale, pick([5, 7, 9]))];
        case 'poly': {
            const n = Math.floor(range(3, 14));
            return [Array.from({ length: n }, () => ({ X: Math.round(cx + range(-10, 10) * scale), Y: Math.round(cy + range(-10, 10) * scale) }))];
        }
        case 'stairs': {
            // Staircase outlines: runs of horizontals at shared heights.
            const g = Math.max(1, Math.round(range(1, 4) * scale));
            const steps = Math.floor(range(2, 7));
            const x0 = Math.round(cx / g) * g, y0 = Math.round(cy / g) * g;
            const up: P[] = [{ X: x0, Y: y0 }];
            for (let i = 0; i < steps; i++) {
                const last = up[up.length - 1];
                up.push({ X: last.X + g, Y: last.Y });
                up.push({ X: last.X + g, Y: last.Y + g * pick([1, 1, 2]) });
            }
            const last = up[up.length - 1];
            up.push({ X: x0, Y: last.Y });
            return rand() < 0.5 ? [up] : [up, up.map((p) => ({ X: p.X + g, Y: p.Y }))];
        }
        case 'comb': {
            // Teeth hanging from a bar: maxima on horizontals, touching tips.
            const g = Math.max(1, Math.round(range(1, 4) * scale));
            const n = Math.floor(range(2, 6));
            const x0 = Math.round(cx / g) * g, y0 = Math.round(cy / g) * g;
            const out: P[] = [{ X: x0, Y: y0 }, { X: x0 + g * (2 * n - 1), Y: y0 }];
            for (let i = n - 1; i >= 0; i--) {
                out.push({ X: x0 + g * (2 * i + 1), Y: y0 + g * 3 });
                out.push({ X: x0 + g * 2 * i, Y: y0 + g * 3 });
                if (i > 0) { out.push({ X: x0 + g * 2 * i, Y: y0 + g }); out.push({ X: x0 + g * (2 * i - 1), Y: y0 + g }); }
            }
            return [out.reverse(), box(x0 - g, y0 + g * 3, x0 + g * 2 * n, y0 + g * 4)];
        }
        case 'copy': {
            const d = disc(cx, cy, range(2, 10) * scale, Math.floor(range(4, 12)));
            return [d, d.map((p) => ({ ...p }))];
        }
    }
}

type Node = { contour: P[]; isHole: boolean; childs: number[] };
function flattenTree(tree: any): { nodes: Node[]; roots: number[]; walk: number[] } {
    const nodes: Node[] = [];
    const ids = new Map<any, number>();
    const visit = (n: any): number => {
        const id = nodes.length;
        ids.set(n, id);
        nodes.push({ contour: n.Contour().map((p: P) => ({ X: p.X, Y: p.Y })), isHole: n.IsHole(), childs: [] });
        nodes[id].childs = n.Childs().map(visit);
        return id;
    };
    const roots = tree.Childs().map(visit);
    // The traversal polygonSet2d walks: GetFirst, then GetNext.
    const walk: number[] = [];
    let node = tree.GetFirst();
    while (node) {
        walk.push(ids.get(node)!);
        node = node.GetNext();
    }
    return { nodes, roots, walk };
}

const cases: unknown[] = [];
for (let k = 0; k < 1500; k++) {
    const scale = pick([1, 10, 1000, 1000, 1000, 3e6]);
    const subject: P[][] = [];
    for (let i = 0; i < Math.floor(range(1, 6)); i++) subject.push(...randomShape(scale));
    const clip: P[][] = [];
    for (let i = 0; i < Math.floor(range(0, 4)); i++) clip.push(...randomShape(scale));
    const op = pick(['union', 'union', 'difference', 'offset'] as const);
    const strictlySimple = rand() < 0.7;
    const record: any = { op, strictlySimple, subject, clip };
    try {
        if (op === 'offset') {
            const delta = pick([1, -1]) * range(0.1, 6) * scale;
            record.delta = delta;
            const co = new ClipperLib.ClipperOffset(2, 2);
            co.AddPaths(subject, ClipperLib.JoinType.jtMiter, ClipperLib.EndType.etClosedPolygon);
            const solution: P[][] = [];
            co.Execute(solution, delta);
            record.result = solution.map((p) => p.map((q) => ({ X: q.X, Y: q.Y })));
        } else {
            const c = new ClipperLib.Clipper();
            c.StrictlySimple = strictlySimple;
            c.AddPaths(subject, ClipperLib.PolyType.ptSubject, true);
            if (op === 'difference') c.AddPaths(clip, ClipperLib.PolyType.ptClip, true);
            const tree = new ClipperLib.PolyTree();
            const ok = c.Execute(op === 'union' ? ClipperLib.ClipType.ctUnion : ClipperLib.ClipType.ctDifference, tree,
                ClipperLib.PolyFillType.pftNonZero, ClipperLib.PolyFillType.pftNonZero);
            record.ok = ok;
            record.tree = flattenTree(tree);
        }
    } catch (e) {
        record.error = e instanceof Error ? `${e.name}: ${e.message}` : String(e);
    }
    cases.push(record);
}
process.stdout.write(JSON.stringify({ cases }));
