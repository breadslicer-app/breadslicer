// Parity oracle for block 9b of the native port: the three.js r181 pieces
// DragonFruit's raft geometry is built from.
//  - earcut: Earcut.triangulate over random outlines with holes, under and
//    over the 80-point hashing threshold, with collinear runs, duplicates,
//    holes touching the outline or each other, and self-intersections;
//  - triangulate: ShapeUtils.triangulateShape (closing duplicates included);
//  - extrude: ExtrudeGeometry without bevels over shapes built the two ways
//    the raft code builds them (polygonSetToShapes and moveTo/lineTo/
//    closePath), then computeVertexNormals and translate;
//  - buffer: an indexed soup through computeVertexNormals, toNonIndexed,
//    mergeGeometries and translate.
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_extrude.json
import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { Earcut } from 'three/src/extras/Earcut.js';

let seed = 0xe7a0;
function rand(): number {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const range = (lo: number, hi: number) => lo + (hi - lo) * rand();
const pick = <T>(xs: readonly T[]): T => xs[Math.floor(rand() * xs.length)];
type P = { x: number; y: number };

// Coordinates as the raft code produces them: Clipper integers over 1000, or
// free doubles.
function q(v: number, quantize: boolean): number {
    return quantize ? Math.round(v * 1000) / 1000 : v;
}
function disc(cx: number, cy: number, r: number, n: number, ccw: boolean, quant: boolean, jitter = 0): P[] {
    const out: P[] = [];
    for (let i = 0; i < n; i++) {
        const a = (ccw ? 1 : -1) * (i / n) * Math.PI * 2;
        const rr = r * (1 + jitter * (rand() - 0.5));
        out.push({ x: q(cx + rr * Math.cos(a), quant), y: q(cy + rr * Math.sin(a), quant) });
    }
    return out;
}
function box(x0: number, y0: number, x1: number, y1: number, ccw: boolean): P[] {
    const b = [{ x: x0, y: y0 }, { x: x1, y: y0 }, { x: x1, y: y1 }, { x: x0, y: y1 }];
    return ccw ? b : b.reverse();
}
function comb(x0: number, y0: number, g: number, n: number): P[] {
    const out: P[] = [{ x: x0, y: y0 }, { x: x0 + g * (2 * n - 1), y: y0 }];
    for (let i = n - 1; i >= 0; i--) {
        out.push({ x: x0 + g * (2 * i + 1), y: y0 + g * 3 });
        out.push({ x: x0 + g * 2 * i, y: y0 + g * 3 });
        if (i > 0) { out.push({ x: x0 + g * 2 * i, y: y0 + g }); out.push({ x: x0 + g * (2 * i - 1), y: y0 + g }); }
    }
    return out;
}
function messUp(ring: P[]): P[] {
    const out: P[] = [];
    for (let i = 0; i < ring.length; i++) {
        const p = ring[i];
        out.push(p);
        if (rand() < 0.08) out.push({ ...p });
        if (rand() < 0.08) {
            const n = ring[(i + 1) % ring.length];
            out.push({ x: (p.x + n.x) / 2, y: (p.y + n.y) / 2 });
        }
    }
    if (rand() < 0.3) out.push({ ...out[0] });
    return out;
}

// An outline and holes; `kind` picks the hard case.
function randomPolygon(): { outer: P[]; holes: P[][] } {
    const quant = rand() < 0.6;
    const cx = range(-50, 50), cy = range(-50, 50);
    const r = range(2, 40);
    const big = rand() < 0.35;
    const n = big ? Math.floor(range(60, 220)) : Math.floor(range(3, 40));
    const kind = pick(['disc', 'disc', 'box', 'comb', 'star', 'touch', 'twin', 'random'] as const);
    let outer: P[];
    switch (kind) {
        case 'box': outer = box(cx - r, cy - r, cx + r, cy + r * range(0.3, 1.5), rand() < 0.8); break;
        case 'comb': outer = comb(q(cx, true), q(cy, true), q(range(0.5, 4), true), Math.floor(range(2, big ? 40 : 8))); break;
        case 'star': {
            outer = [];
            for (let i = 0; i < n; i++) {
                const a = (i / n) * Math.PI * 2;
                const rr = i % 2 ? r : r * range(0.2, 0.7);
                outer.push({ x: q(cx + rr * Math.cos(a), quant), y: q(cy + rr * Math.sin(a), quant) });
            }
            break;
        }
        case 'random':
            outer = Array.from({ length: Math.floor(range(3, 12)) }, () => ({ x: q(cx + range(-r, r), quant), y: q(cy + range(-r, r), quant) }));
            break;
        default: outer = disc(cx, cy, r, n, rand() < 0.8, quant, rand() < 0.5 ? 0.3 : 0);
    }
    const holes: P[][] = [];
    const nh = kind === 'comb' || kind === 'random' ? 0 : Math.floor(range(0, big ? 8 : 4));
    for (let h = 0; h < nh; h++) {
        const a = range(0, Math.PI * 2), d = range(0, r * 0.5);
        const hr = r * range(0.05, 0.2);
        holes.push(disc(cx + d * Math.cos(a), cy + d * Math.sin(a), hr, Math.floor(range(3, big ? 40 : 12)), rand() < 0.2, quant));
    }
    if (kind === 'touch' && outer.length > 3) {
        // A hole whose vertex sits on an outline vertex.
        const v = outer[0];
        holes.push([{ ...v }, { x: q(v.x - r * 0.2, quant), y: q(v.y + r * 0.05, quant) }, { x: q(v.x - r * 0.2, quant), y: q(v.y - r * 0.05, quant) }]);
    }
    if (kind === 'twin') {
        // Two holes sharing their leftmost point (compareXYSlope's tie).
        const lx = q(cx - r * 0.3, quant), ly = q(cy, quant);
        holes.push([{ x: lx, y: ly }, { x: q(lx + r * 0.2, quant), y: q(ly + r * 0.1, quant) }, { x: q(lx + r * 0.1, quant), y: q(ly + r * 0.25, quant) }]);
        holes.push([{ x: lx, y: ly }, { x: q(lx + r * 0.1, quant), y: q(ly - r * 0.25, quant) }, { x: q(lx + r * 0.2, quant), y: q(ly - r * 0.1, quant) }]);
    }
    return {
        outer: rand() < 0.3 ? messUp(outer) : outer,
        holes: holes.map((h) => (rand() < 0.2 ? messUp(h) : h)),
    };
}

const v2 = (ps: P[]) => ps.map((p) => new THREE.Vector2(p.x, p.y));
const plain = (ps: THREE.Vector2[]) => ps.map((p) => ({ x: p.x, y: p.y }));
// Small attributes in full; long ones as their length and an FNV-1a over the
// f32 bits (the fixture would otherwise run to tens of megabytes).
function packF32(a: Float32Array) {
    if (a.length <= 600) return Array.from(a);
    let h = 2166136261;
    const u = new Uint32Array(a.buffer, a.byteOffset, a.length);
    for (let i = 0; i < u.length; i++) { h ^= u[i]; h = Math.imul(h, 16777619); }
    return { len: a.length, fnv: h >>> 0 };
}
function dumpGeometry(g: THREE.BufferGeometry) {
    const attr = (name: string) => {
        const a = g.getAttribute(name);
        return a ? packF32(a.array as Float32Array) : null;
    };
    return {
        position: attr('position'),
        normal: attr('normal'),
        uv: attr('uv'),
        index: g.index ? Array.from(g.index.array) : null,
        groups: g.groups.map((gr) => ({ start: gr.start, count: gr.count, materialIndex: gr.materialIndex })),
    };
}

const earcutCases: unknown[] = [];
for (let k = 0; k < 600; k++) {
    const { outer, holes } = randomPolygon();
    // One-point holes: earcut's Steiner points.
    if (rand() < 0.15) {
        const c = outer[0], d = outer[Math.floor(outer.length / 2)];
        for (let s = 0; s < Math.floor(range(1, 4)); s++) {
            const t = range(0.2, 0.8);
            holes.push([{ x: c.x + (d.x - c.x) * t, y: c.y + (d.y - c.y) * t }]);
        }
    }
    const data: number[] = [];
    const holeIndices: number[] = [];
    for (const p of outer) data.push(p.x, p.y);
    for (const h of holes) {
        holeIndices.push(data.length / 2);
        for (const p of h) data.push(p.x, p.y);
    }
    const record: any = { data, holeIndices };
    try {
        record.triangles = Earcut.triangulate(data, holeIndices.length ? holeIndices : undefined);
    } catch (e) {
        record.error = String(e);
    }
    earcutCases.push(record);
}

const triangulateCases: unknown[] = [];
for (let k = 0; k < 200; k++) {
    const { outer, holes } = randomPolygon();
    const contour = v2(outer), hs = holes.map(v2);
    const record: any = { contour: outer, holes };
    try {
        record.faces = THREE.ShapeUtils.triangulateShape(contour, hs);
        record.contourAfter = plain(contour);
        record.holesAfter = hs.map(plain);
    } catch (e) {
        record.error = String(e);
    }
    triangulateCases.push(record);
}

const extrudeCases: unknown[] = [];
for (let k = 0; k < 250; k++) {
    const count = rand() < 0.7 ? 1 : Math.floor(range(2, 4));
    const style = pick(['set', 'pen'] as const);
    const polys = Array.from({ length: count }, randomPolygon);
    const depth = pick([0.5, 1, 1.2, range(0.1, 3)]);
    const translateZ = rand() < 0.5 ? range(0, 2) : null;
    const record: any = { style, polys, depth, translateZ };
    try {
        const shapes = polys.map(({ outer, holes }) => {
            if (style === 'set') {
                // polygonSetToShapes.
                const shape = new THREE.Shape(v2(outer));
                for (const h of holes) {
                    const path = new THREE.Path(v2(h));
                    path.closePath();
                    shape.holes.push(path);
                }
                return shape;
            }
            // generatePerimeterWall.
            const shape = new THREE.Shape();
            shape.moveTo(outer[0].x, outer[0].y);
            for (let i = 1; i < outer.length; i++) shape.lineTo(outer[i].x, outer[i].y);
            shape.closePath();
            for (const h of holes) {
                const path = new THREE.Path();
                path.moveTo(h[0].x, h[0].y);
                for (let i = 1; i < h.length; i++) path.lineTo(h[i].x, h[i].y);
                path.closePath();
                shape.holes.push(path);
            }
            return shape;
        });
        const geom = new THREE.ExtrudeGeometry(shapes, { depth, bevelEnabled: false, curveSegments: 24 });
        geom.computeVertexNormals();
        if (translateZ !== null) geom.translate(0, 0, translateZ);
        record.geometry = dumpGeometry(geom);
    } catch (e) {
        record.error = String(e);
    }
    extrudeCases.push(record);
}

// Indexed soups: a grid of quads lifted into a bumpy sheet.
const bufferCases: unknown[] = [];
for (let k = 0; k < 40; k++) {
    const parts: { positions: number[]; index: number[] }[] = [];
    for (let m = 0; m < Math.floor(range(1, 4)); m++) {
        const w = Math.floor(range(2, 6)), h = Math.floor(range(2, 6));
        const positions: number[] = [], index: number[] = [];
        for (let j = 0; j <= h; j++) for (let i = 0; i <= w; i++) positions.push(i * range(0.5, 1.5), j * range(0.5, 1.5), range(-1, 1));
        for (let j = 0; j < h; j++) for (let i = 0; i < w; i++) {
            const a = j * (w + 1) + i, b = a + 1, c = a + w + 1, d = c + 1;
            index.push(a, b, d, a, d, c);
        }
        parts.push({ positions, index });
    }
    const tz = range(-2, 2);
    const geoms = parts.map(({ positions, index }) => {
        const g = new THREE.BufferGeometry();
        g.setAttribute('position', new THREE.Float32BufferAttribute(positions, 3));
        g.setIndex(index);
        g.computeVertexNormals();
        return g;
    });
    const indexed = geoms.map(dumpGeometry);
    const nonIndexed = geoms.map((g) => g.toNonIndexed());
    const merged = mergeGeometries(nonIndexed, false)!;
    const mergedDump = dumpGeometry(merged);
    merged.computeVertexNormals();
    merged.translate(0.25, -0.5, tz);
    bufferCases.push({ parts, tz, indexed, merged: mergedDump, final: dumpGeometry(merged) });
}

process.stdout.write(JSON.stringify({ earcut: earcutCases, triangulate: triangulateCases, extrude: extrudeCases, buffer: bufferCases }));
