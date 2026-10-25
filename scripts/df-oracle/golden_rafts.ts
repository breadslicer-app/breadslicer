// Parity oracle for block 9c of the native port: DragonFruit's crenelated raft
// generator (src/supports/Rafts/Crenelated/geometry) run on generated scenes.
//  - sets: polygonSet2d union / difference / offset (islands inside holes
//    included, so the PolyTree walk's repeated visits are recorded);
//  - raster: rasterizePolygonsToRegion over triangle soups;
//  - plate: collectModelPlateFootprint over sphere / box / torus models,
//    plate-facing and tilted, indexed and not;
//  - rings: insetConvexPolygon, computeRaftOuterBoundary, computeFootprint;
//  - edges: buildLineRaftEdgePairs (backbone on and off);
//  - lines: generateUnionedLineRaftMesh with border rings and chamfers, and
//    generateChamferedBeam;
//  - export: the raft step of ExportManager.exportScene (`// 4. Add Raft`),
//    transcribed below around DragonFruit's own functions (exportScene itself
//    needs Tauri), over roots, per-model settings and plate models.
// Every model gets its own geometry: DragonFruit's world-shadow cache keys
// omit position X/Y (see SC-76), which shared geometry would expose.
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_rafts.json
import * as THREE from 'three';
import { DEFAULT_RAFT_SETTINGS } from './src/supports/Rafts/Crenelated/RaftDefaults';
import { buildLineRaftEdgePairs } from './src/supports/Rafts/Crenelated/geometry/buildLineRaftEdgePairs';
import { buildRaftFootprintMeshes } from './src/supports/Rafts/Crenelated/geometry/generateRaftFromFootprint';
import { collectModelPlateFootprint } from './src/supports/Rafts/Crenelated/geometry/modelPlateFootprint';
import { inflateModelPlateClearance, raftBandTopMm } from './src/supports/Rafts/Crenelated/geometry/computeRaftFootprint';
import { filterLineRaftEdges, generateUnionedLineRaftMesh } from './src/supports/Rafts/Crenelated/geometry/generateUnionedLineRaftMesh';
import { generateChamferedBeam } from './src/supports/Rafts/Crenelated/geometry/generateChamferedBeam';
import { computeFootprint } from './src/supports/Rafts/Crenelated/geometry/computeFootprint';
import { computeRaftOuterBoundary } from './src/supports/Rafts/Crenelated/geometry/computeRaftOuterBoundary';
import { insetConvexPolygon } from './src/supports/Rafts/Crenelated/geometry/insetConvexPolygon';
import { rasterizePolygonsToRegion } from './src/supports/Rafts/Crenelated/geometry/polygonRaster2d';
import { differencePolygonSets, offsetPolygonSet, unionPolygonSets } from './src/supports/Rafts/Crenelated/geometry/polygonSet2d';

let seed = 0x9ca5;
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
const V = (ps: P[]) => ps.map((p) => new THREE.Vector2(p.x, p.y));
const plain = (ps: { x: number; y: number }[]) => ps.map((p) => ({ x: p.x, y: p.y }));
const plainSet = (polys: any[]) => polys.map((p) => ({ outer: plain(p.outer), holes: p.holes.map(plain) }));
const vset = (polys: { outer: P[]; holes: P[][] }[]) => polys.map((p) => ({ outer: V(p.outer), holes: p.holes.map(V) }));

function packF32(a: ArrayLike<number>) {
    const f = a instanceof Float32Array ? a : Float32Array.from(a as number[]);
    if (f.length <= 600) return Array.from(f);
    let h = 2166136261;
    const u = new Uint32Array(f.buffer, f.byteOffset, f.length);
    for (let i = 0; i < u.length; i++) { h ^= u[i]; h = Math.imul(h, 16777619); }
    return { len: f.length, fnv: h >>> 0 };
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

function disc(cx: number, cy: number, r: number, n: number, ccw = true, jitter = 0): P[] {
    const out: P[] = [];
    for (let i = 0; i < n; i++) {
        const a = (ccw ? 1 : -1) * (i / n) * Math.PI * 2;
        const rr = r * (1 + jitter * (rand() - 0.5));
        out.push({ x: cx + rr * Math.cos(a), y: cy + rr * Math.sin(a) });
    }
    return out;
}
function randomSet(): { outer: P[]; holes: P[][] }[] {
    const out: { outer: P[]; holes: P[][] }[] = [];
    for (let k = 0; k < Math.floor(range(1, 4)); k++) {
        const cx = range(-15, 15), cy = range(-15, 15), r = range(2, 12);
        const kind = pick(['disc', 'ring', 'island', 'box', 'halfmicron'] as const);
        if (kind === 'halfmicron') {
            // Coordinates on a half-micron grid: x * 1000 lands on .5, where
            // Math.round and round-half-away differ for negatives.
            const h = (v: number) => Math.round(v * 2000) / 2000;
            const w = range(1, 10), hh = range(1, 10);
            out.push({ outer: [{ x: h(cx), y: h(cy) }, { x: h(cx + w), y: h(cy) }, { x: h(cx + w), y: h(cy + hh) }, { x: h(cx), y: h(cy + hh) }], holes: [] });
        } else if (kind === 'box') {
            const w = range(1, 10), h = range(1, 10);
            out.push({ outer: [{ x: cx, y: cy }, { x: cx + w, y: cy }, { x: cx + w, y: cy + h }, { x: cx, y: cy + h }], holes: [] });
        } else if (kind === 'disc') {
            out.push({ outer: disc(cx, cy, r, Math.floor(range(5, 30)), true, rand() < 0.5 ? 0.4 : 0), holes: [] });
        } else {
            out.push({ outer: disc(cx, cy, r, Math.floor(range(8, 30))), holes: [disc(cx, cy, r * range(0.4, 0.8), Math.floor(range(6, 20)), false)] });
            if (kind === 'island') out.push({ outer: disc(cx, cy, r * range(0.1, 0.3), Math.floor(range(5, 12))), holes: [] });
        }
    }
    return out;
}

// --- Models standing on (or near) the plate.
type ModelRecord = { positions: number[]; index: number[] | null; center: number[]; transform: any; visible: boolean | null };
function makeModel(at?: { x: number; y: number }): { record: ModelRecord; source: any } {
    const size = range(3, 14);
    const kind = pick(['sphere', 'box', 'torus', 'cylinder'] as const);
    let g: THREE.BufferGeometry;
    switch (kind) {
        case 'sphere': g = new THREE.SphereGeometry(size / 2, Math.floor(range(8, 24)), Math.floor(range(6, 14))); break;
        case 'box': g = new THREE.BoxGeometry(size, size * range(0.4, 1.5), size * range(0.3, 1.2)); break;
        case 'torus': g = new THREE.TorusGeometry(size / 2, size * range(0.1, 0.25), 8, Math.floor(range(12, 28))); break;
        default: g = new THREE.CylinderGeometry(size / 2, size * range(0.2, 0.6), size, Math.floor(range(8, 20))); break;
    }
    if (rand() < 0.4) g = g.toNonIndexed();
    // Local geometry offset from the origin; `center` re-centres it.
    // Flush: an unrotated box resting exactly on the plate (vertices at z = 0).
    const flush = !!at && kind === 'box' && rand() < 0.5;
    const off = [range(-5, 5), range(-5, 5), flush ? 0 : range(-5, 5)];
    g.translate(off[0], off[1], off[2]);
    const center = rand() < 0.7 ? new THREE.Vector3(off[0], off[1], off[2]) : new THREE.Vector3(range(-6, 6), range(-6, 6), range(-6, 6));
    const tilted = rand() < 0.35;
    const rotation = { x: tilted ? range(-1.2, 1.2) : 0, y: tilted ? range(-1.2, 1.2) : 0, z: range(-3.1, 3.1) };
    const scale = new THREE.Vector3(pick([1, 1, range(0.5, 2)]), pick([1, 1, range(0.5, 2)]), pick([1, 1, range(0.5, 2), -1]));
    // Somewhere between sitting on the plate and lifted clear of the raft.
    if (flush) { rotation.x = 0; rotation.y = 0; scale.set(1, 1, 1); center.set(off[0], off[1], off[2]); }
    const position = at
        ? new THREE.Vector3(at.x + range(-3, 3), at.y + range(-3, 3), flush ? (g.boundingBox ?? (g.computeBoundingBox(), g.boundingBox!)).max.z - off[2] : range(-0.2, 0.6) * size * 0.5)
        : new THREE.Vector3(range(-20, 20), range(-20, 20), range(-0.3, 1.2) * size * 0.5 + pick([0, 0, 0, size]));
    const visible = at ? true : pick([true, true, true, false, null] as const);
    const transform = { position, rotation, scale };
    const pos = Array.from(g.getAttribute('position').array as Float32Array);
    return {
        record: { positions: pos, index: g.index ? Array.from(g.index.array) : null, center: [center.x, center.y, center.z], transform: { position: [position.x, position.y, position.z], rotation: [rotation.x, rotation.y, rotation.z], scale: [scale.x, scale.y, scale.z] }, visible },
        source: { geometry: { geometry: g, center }, transform, ...(visible === null ? {} : { visible }) },
    };
}

function randomSettings(): any {
    const s: any = { ...DEFAULT_RAFT_SETTINGS };
    s.bottomMode = pick(['solid', 'solid', 'line', 'off'] as const);
    s.wallEnabled = rand() < 0.6;
    s.thickness = pick([0.5, range(0.1, 2), 0]);
    s.chamferAngle = pick([45, 90, range(45, 90), range(30, 100)]);
    s.wallHeight = pick([0.35, range(0, 2)]);
    s.wallThickness = pick([0.5, range(0, 2)]);
    s.crenulationGapWidth = pick([1.5, 0, range(0.2, 3)]);
    s.crenulationSpacing = pick([5, 0, range(1, 8)]);
    s.lineWidthMm = pick([1.5, range(0.3, 3)]);
    s.lineHeightMm = pick([0.6, range(0.05, 2)]);
    return s;
}

// The raft step of ExportManager.exportScene, verbatim but for its inputs.
function exportRafts(roots: any[], scoped: Set<string> | null, global: any, perModel: Map<string, any>, models: any[]) {
    const out: any[] = [];
    const hasScopedModelFilter = !!scoped;
    const scopedModelIds = scoped ?? new Set<string>();
    const getRaftSettingsForModel = (id: string | null) => (!id ? global : perModel.get(id) ?? global);
    if (global.bottomMode !== 'off') {
        const allRoots = roots;
        const rootsByModel = new Map<string, typeof allRoots>();
        for (const root of allRoots) {
            const rootModelId = root.modelId ?? null;
            if (hasScopedModelFilter) {
                if (!rootModelId || !scopedModelIds.has(rootModelId)) continue;
            }
            const mid = rootModelId ?? '__orphan__';
            let arr = rootsByModel.get(mid);
            if (!arr) { arr = []; rootsByModel.set(mid, arr); }
            arr.push(root);
        }
        const clearanceTargets = models.filter((model) => model.visible);
        for (const [modelKey, roots] of rootsByModel) {
            if (roots.length === 0) continue;
            const modelId = modelKey === '__orphan__' ? null : modelKey;
            const raftSettings = modelId ? getRaftSettingsForModel(modelId) : global;
            const circles = roots.map((r: any) => ({ x: r.transform.pos.x, y: r.transform.pos.y, r: r.diameter / 2 }));
            const clearance = collectModelPlateFootprint(clearanceTargets, raftBandTopMm(raftSettings));
            const parts = buildRaftFootprintMeshes({ circles, raft: raftSettings, clearance });
            if (!parts.baseMesh && parts.footprint.length === 0) continue;
            const meshes: THREE.BufferGeometry[] = [];
            if (parts.baseMesh) meshes.push(parts.baseMesh.geometry);
            if (raftSettings.bottomMode === 'line') {
                const nodes2d = roots.map((r: any) => new THREE.Vector2(r.transform.pos.x, r.transform.pos.y));
                const hasBorderRing = parts.footprint.length > 0;
                const edgePairs = buildLineRaftEdgePairs(nodes2d, { hasBorderRing, keepFactor: 8, absMaxLen: 220, enforceConnected: true });
                const beamHeight = Math.max(0.01, raftSettings.lineHeightMm);
                const unionEdges: Array<[THREE.Vector2, THREE.Vector2]> = filterLineRaftEdges(
                    edgePairs.map(([a, b]) => [nodes2d[a], nodes2d[b]]),
                    inflateModelPlateClearance(clearance),
                    raftSettings.lineWidthMm,
                );
                const unionMesh = generateUnionedLineRaftMesh(unionEdges, { widthMm: raftSettings.lineWidthMm, heightMm: beamHeight, borderProfile: null });
                const unionPositionAttribute = unionMesh.geometry.getAttribute('position');
                if (unionPositionAttribute && unionPositionAttribute.count > 0) meshes.push(unionMesh.geometry);
                else for (const [a, b] of unionEdges) {
                    meshes.push(generateChamferedBeam(new THREE.Vector3(a.x, a.y, 0), new THREE.Vector3(b.x, b.y, 0), { widthMm: raftSettings.lineWidthMm, heightMm: beamHeight, chamferAngleDeg: 90 }).geometry);
                }
            }
            if (parts.wallMesh) meshes.push(parts.wallMesh.geometry);
            out.push({ modelKey, footprint: plainSet(parts.footprint), clearance: plainSet(clearance), meshes: meshes.map(dumpGeometry) });
        }
    }
    return out;
}

const guard = (record: any, f: () => void) => {
    try { f(); } catch (e) { record.error = e instanceof Error ? `${e.name}: ${e.message}` : String(e); }
};

const sets: unknown[] = [];
for (let k = 0; k < 160; k++) {
    const subject = randomSet(), clip = randomSet();
    const op = pick(['union', 'difference', 'offset'] as const);
    const delta = pick([1, -1]) * range(0.05, 3);
    const record: any = { op, subject, clip, delta };
    guard(record, () => {
        record.result = plainSet(op === 'union' ? unionPolygonSets(vset(subject))
            : op === 'difference' ? differencePolygonSets(vset(subject), vset(clip))
                : offsetPolygonSet(vset(subject), delta));
    });
    sets.push(record);
}

const raster: unknown[] = [];
for (let k = 0; k < 30; k++) {
    const soup: P[][] = [];
    const cx = range(-10, 10), cy = range(-10, 10), r = range(1, 25);
    for (let i = 0; i < Math.floor(range(1, 200)); i++) {
        const a = range(0, Math.PI * 2), d = r * Math.sqrt(rand());
        const x = cx + d * Math.cos(a), y = cy + d * Math.sin(a), s = range(0.05, r * 0.4);
        soup.push([{ x, y }, { x: x + s * range(-1, 1), y: y + s * range(0.2, 1) }, { x: x + s * range(-1, 1), y: y - s * range(0.2, 1) }]);
    }
    if (rand() < 0.3) soup.push(disc(cx, cy, r * 1.2, 24), disc(cx, cy, r * 0.5, 16));
    raster.push({ soup, result: plainSet(rasterizePolygonsToRegion(soup.map(V))) });
}

const plate: unknown[] = [];
for (let k = 0; k < 40; k++) {
    const made = Array.from({ length: Math.floor(range(1, 4)) }, () => makeModel());
    const band = pick([0.85, range(0.1, 3), 0]);
    const record: any = { models: made.map((m) => m.record), band };
    guard(record, () => { record.result = plainSet(collectModelPlateFootprint(made.map((m) => m.source), band)); });
    plate.push(record);
}

const rings: unknown[] = [];
for (let k = 0; k < 60; k++) {
    const ring = rand() < 0.7 ? disc(range(-10, 10), range(-10, 10), range(1, 15), Math.floor(range(3, 40)), rand() < 0.8, rand() < 0.3 ? 0.3 : 0)
        : pick([
            [{ x: 0, y: 0 }, { x: 5, y: 0 }, { x: 5, y: 0 }, { x: 5, y: 5 }, { x: 2.5, y: 5 }, { x: 0, y: 5 }],
            // A spike folding straight back: opposite neighbouring normals.
            [{ x: 0, y: 0 }, { x: 10, y: 0 }, { x: 5, y: 0 }, { x: 5, y: 5 }],
            // A needle tip: the outward offset's 0.1 floor.
            [{ x: 0, y: 0 }, { x: 10, y: 0.3 }, { x: 0, y: 0.6 }, { x: -2, y: 0.3 }],
        ]);
    const d = pick([0.5, -0.5, range(-3, 3), 1e-6]);
    const settings = { ...DEFAULT_RAFT_SETTINGS, wallThickness: pick([0.5, 0, range(0, 3)]) };
    const circles = Array.from({ length: Math.floor(range(0, 8)) }, () => ({ x: range(-10, 10), y: range(-10, 10), r: range(0, 2) }));
    const margin = pick([undefined, range(0, 2)]);
    const samples = pick([undefined, 8, 12.7, 3]);
    rings.push({
        ring, d, settings, circles, margin: margin ?? null, samples: samples ?? null,
        inset: plain(insetConvexPolygon(V(ring), d)),
        outerBoundary: plain(computeRaftOuterBoundary(V(ring), settings as any)),
        footprint: plain(computeFootprint(circles, { ...(margin === undefined ? {} : { marginMm: margin }), ...(samples === undefined ? {} : { samplesPerCircle: samples }) })),
    });
}

const edges: unknown[] = [];
for (let k = 0; k < 60; k++) {
    const n = Math.floor(range(0, 40));
    const nodes: P[] = [];
    for (let i = 0; i < n; i++) {
        if (i > 0 && rand() < 0.05) { nodes.push({ ...nodes[i - 1] }); continue; }
        nodes.push({ x: Math.round(range(-30, 30) * 10) / (rand() < 0.5 ? 10 : 1), y: Math.round(range(-30, 30) * 10) / 10 });
    }
    // A far cluster: the long bridges.
    if (rand() < 0.3) for (let i = 0; i < 4; i++) nodes.push({ x: 300 + range(0, 5), y: range(0, 5) });
    const options = { hasBorderRing: rand() < 0.5, keepFactor: pick([undefined, 8, 2, 0.5]), absMaxLen: pick([undefined, 220, 10]), enforceConnected: pick([undefined, true, false]), includeSpanningTreeBackbone: pick([undefined, true, false]) };
    edges.push({ nodes, options: JSON.parse(JSON.stringify(options)), result: buildLineRaftEdgePairs(V(nodes), options as any) });
}

const lines: unknown[] = [];
for (let k = 0; k < 40; k++) {
    const nodes = Array.from({ length: Math.floor(range(2, 14)) }, () => ({ x: range(-20, 20), y: range(-20, 20) }));
    const pairs = buildLineRaftEdgePairs(V(nodes), { hasBorderRing: false });
    const edgeList: [P, P][] = pairs.map(([a, b]) => [nodes[a], nodes[b]]);
    if (rand() < 0.2) edgeList.push([nodes[0], { ...nodes[0] }]);
    const widthMm = pick([1.5, range(0.2, 3)]), heightMm = pick([0.6, range(0.05, 2)]);
    const border = rand() < 0.4 ? disc(0, 0, range(15, 30), Math.floor(range(6, 24))) : null;
    const chamfer = pick([undefined, 90, 60, 45, 30, NaN]);
    const clearance = rand() < 0.5 ? vset(randomSet()) : null;
    const record: any = { edges: edgeList, widthMm, heightMm, border, chamfer: chamfer === undefined ? null : Number.isNaN(chamfer) ? 'NaN' : chamfer, clearance: clearance ? plainSet(clearance) : null };
    guard(record, () => {
        record.filtered = filterLineRaftEdges(edgeList.map(([a, b]) => [new THREE.Vector2(a.x, a.y), new THREE.Vector2(b.x, b.y)]), clearance, widthMm).map(([a, b]) => [{ x: a.x, y: a.y }, { x: b.x, y: b.y }]);
        record.mesh = dumpGeometry(generateUnionedLineRaftMesh(edgeList.map(([a, b]) => [new THREE.Vector2(a.x, a.y), new THREE.Vector2(b.x, b.y)]), { widthMm, heightMm, borderProfile: border ? V(border) : null, ...(chamfer === undefined ? {} : { chamferAngleDeg: chamfer }) }).geometry);
        const [a, b] = edgeList[0];
        const z = rand() < 0.3 ? range(-1, 1) : 0;
        record.beamArgs = { a, b, z, angle: chamfer ?? 90 };
        record.beam = dumpGeometry(generateChamferedBeam(new THREE.Vector3(a.x, a.y, z), new THREE.Vector3(b.x, b.y, -z), { widthMm, heightMm, chamferAngleDeg: Number.isNaN(chamfer) ? 90 : chamfer ?? 90 }).geometry);
    });
    lines.push(record);
}

const exportCases: unknown[] = [];
for (let k = 0; k < 120; k++) {
    const made = Array.from({ length: Math.floor(range(0, 3)) }, () => makeModel());
    const modelIds = ['m1', 'm2', 'm3'];
    const roots: any[] = [];
    const clusterCount = Math.floor(range(1, 4));
    for (let c = 0; c < clusterCount; c++) {
        const cx = range(-25, 25), cy = range(-25, 25), spread = range(1, 12);
        // A model standing on the plate among the roots: the trimmed footprint.
        if (rand() < 0.5) made.push(makeModel({ x: cx, y: cy }));
        const owner = pick([...modelIds, null, '']);
        for (let i = 0; i < Math.floor(range(1, 25)); i++) {
            roots.push({ modelId: owner, transform: { pos: { x: cx + range(-spread, spread), y: cy + range(-spread, spread) } }, diameter: pick([1, 2, range(0.5, 4)]) });
        }
    }
    const global = randomSettings();
    if (global.bottomMode === 'off' && rand() < 0.7) global.bottomMode = 'solid';
    const perModel = new Map<string, any>();
    for (const id of modelIds) if (rand() < 0.3) perModel.set(id, randomSettings());
    const scoped = rand() < 0.25 ? new Set(modelIds.slice(0, Math.floor(range(1, 3)))) : null;
    const record: any = { roots, scoped: scoped ? [...scoped] : null, global, perModel: Object.fromEntries(perModel), models: made.map((m) => m.record) };
    guard(record, () => { record.groups = exportRafts(roots, scoped, global, perModel, made.map((m) => m.source)); });
    exportCases.push(record);
}

process.stdout.write(JSON.stringify({ sets, raster, plate, rings, edges, lines, export: exportCases }));
