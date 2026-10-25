// Parity oracle for block 10 of the native port: the support tessellation the
// slicer rasterises, `buildSupportAndRaftWorldTriangles` of
// src/features/slicing/rasterLayerZipExport.ts, run on DragonFruit's own
// support states: the 52 braced forests of block 8c (fixture df_islands_c.json)
// and the 90 auto-brace runs of block 7 (df_autobrace.json, with kickstands).
// Each case varies the visible models, the tip shrink, the global raft
// settings and, for the 8c scenes, the scene mesh as plate clearance. Auto-
// support never curves a segment, so a fraction of the cases turn segments
// and braces into Beziers first (the modified state is recorded in full).
// The tip penetration stays 0: no material/printer profile is active in Node.
// Inputs: FIX_C (df_islands_c.json), FIX_N (df_native_islands.json),
// FIX_B (df_autobrace.json).
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_support_triangles.json
import fs from 'node:fs';
import * as THREE from 'three';
import { setSnapshot } from './src/supports/state';
import { getRaftSettings, setRaftSettings } from './src/supports/Rafts/Crenelated/RaftState';
import { DEFAULT_RAFT_SETTINGS } from './src/supports/Rafts/Crenelated/RaftDefaults';

let seed = 0x10ad;
function rand(): number {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const range = (lo: number, hi: number) => lo + (hi - lo) * rand();
const pick = <T>(xs: readonly T[]): T => xs[Math.floor(rand() * xs.length)];
const KEYS = ['roots', 'trunks', 'branches', 'leaves', 'twigs', 'sticks', 'braces', 'stumps', 'kickstands', 'knots'];

function digest(values: number[]) {
    const f = Float32Array.from(values);
    let h = 2166136261;
    const u = new Uint32Array(f.buffer);
    for (let i = 0; i < u.length; i++) { h ^= u[i]; h = Math.imul(h, 16777619); }
    return { len: f.length, fnv: h >>> 0, head: Array.from(f.subarray(0, 90)) };
}

function randomRaft(): any {
    const s: any = { ...DEFAULT_RAFT_SETTINGS };
    s.bottomMode = pick(['solid', 'solid', 'line', 'off'] as const);
    s.wallEnabled = rand() < 0.5;
    s.thickness = pick([0.5, range(0.2, 1.5)]);
    s.chamferAngle = pick([45, 90, range(45, 90)]);
    s.wallHeight = pick([0.35, range(0.1, 1.5)]);
    s.wallThickness = pick([0.5, range(0.2, 1.2)]);
    s.crenulationGapWidth = pick([1.5, 0]);
    s.crenulationSpacing = pick([5, 0]);
    s.lineWidthMm = pick([1.5, range(0.5, 2.5)]);
    s.lineHeightMm = pick([0.6, range(0.2, 1.2)]);
    return s;
}

// Curve some shafts and braces: control points a third and two thirds along,
// pushed sideways, as a user's curve edit leaves them.
function curve(state: any) {
    const bend = (a: any, b: any) => {
        const off = { x: range(-2, 2), y: range(-2, 2), z: range(-1, 1) };
        const lerp = (t: number) => ({ x: a.x + (b.x - a.x) * t + off.x, y: a.y + (b.y - a.y) * t + off.y, z: a.z + (b.z - a.z) * t + off.z });
        return { controlPoint1: lerp(1 / 3), controlPoint2: lerp(2 / 3), startTangent: { x: 0, y: 0, z: 1 }, endTangent: { x: 0, y: 0, z: 1 }, tension: 0.5, bias: 0.5, resolution: 12 };
    };
    for (const key of ['trunks', 'branches', 'twigs', 'sticks', 'stumps', 'kickstands']) {
        for (const e of Object.values(state[key] ?? {}) as any[]) {
            for (const seg of e.segments ?? []) {
                if (rand() < 0.4 && seg.bottomJoint && seg.topJoint) Object.assign(seg, { type: 'bezier' }, bend(seg.bottomJoint.pos, seg.topJoint.pos));
            }
        }
    }
    for (const b of Object.values(state.braces ?? {}) as any[]) {
        const s = state.knots[b.startKnotId], e = state.knots[b.endKnotId];
        if (s && e && rand() < 0.4) b.curve = { type: 'bezier', ...bend(s.pos, e.pos) };
    }
}

function soupSource(soup: number[], visible: boolean | undefined) {
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.Float32BufferAttribute(soup, 3));
    return {
        geometry: { geometry: g, center: new THREE.Vector3(0, 0, 0) },
        transform: { position: new THREE.Vector3(0, 0, 0), rotation: { x: 0, y: 0, z: 0 }, scale: new THREE.Vector3(1, 1, 1) },
        ...(visible === undefined ? {} : { visible }),
    };
}

// Orphan some roots: drop a share of the trunks and keep their roots, which
// then draw with the fallback cone radius and key their raft by the root.
function orphan(state: any) {
    for (const [id, t] of Object.entries(state.trunks ?? {}) as Array<[string, any]>) {
        if (rand() < 0.3) {
            delete state.trunks[id];
            for (const k of Object.keys(state.knots)) if (state.knots[k].parentShaftId === id) delete state.knots[k];
        }
    }
}

async function main() {
// A copy of the module that also exports its contact primitives, so they can
// be driven with a tip penetration (no profile is active in Node).
const source = fs.readFileSync('./src/features/slicing/rasterLayerZipExport.ts', 'utf8');
fs.writeFileSync('./src/features/slicing/_rasterOracle.ts', `${source}
export { appendContactConePrimitive, appendContactDiskPrimitive };
`);
const mod: any = await import('./src/features/slicing/_rasterOracle');
const { buildSupportAndRaftWorldTriangles, appendContactConePrimitive, appendContactDiskPrimitive } = mod;
const flat = (tris: any[]) => tris.flatMap((t: any) => [t.ax, t.ay, t.az, t.bx, t.by, t.bz, t.cx, t.cy, t.cz]);

const fixC = JSON.parse(fs.readFileSync(process.env.FIX_C!, 'utf8'));
const fixN = JSON.parse(fs.readFileSync(process.env.FIX_N!, 'utf8'));
const fixB = JSON.parse(fs.readFileSync(process.env.FIX_B!, 'utf8'));
type Source = { from: 'islands_c' | 'autobrace'; index: number; scene: string; state: any };
const sources: Source[] = [];
fixC.runs.forEach((r: any, i: number) => { if (r.result) sources.push({ from: 'islands_c', index: i, scene: fixN.runs[r.run].scene, state: r.result.support }); });
fixB.runs.forEach((r: any, i: number) => { if (r.result) sources.push({ from: 'autobrace', index: i, scene: r.scene, state: r.result.support }); });

const cases: unknown[] = [];
for (let k = 0; k < 120; k++) {
    const src = sources[k < sources.length ? k : Math.floor(rand() * sources.length)];
    const state = structuredClone(src.state);
    const curved = rand() < 0.3;
    if (curved) curve(state);
    const orphaned = rand() < 0.2;
    if (orphaned) orphan(state);
    const visibleMode = pick(['model', 'model', 'model', 'none', 'other'] as const);
    const visible = new Set(visibleMode === 'model' ? [src.scene] : visibleMode === 'other' ? ['someone-else'] : []);
    const shrink = pick([0, 0, 25, range(0, 60)]);
    const raft = randomRaft();
    const clearance = src.from === 'islands_c' && rand() < 0.6;
    const record: any = { from: src.from, index: src.index, scene: src.scene, visible: [...visible], shrink, raft, clearance };
    if (curved || orphaned) record.state = state;
    // A plain state with every collection as an own key: `setSnapshot`
    // rebuilds its `supports` map (and the type views) from these.
    const base: any = { selectedId: null, hoveredId: null };
    for (const key of KEYS) base[key] = structuredClone(state[key] ?? {});
    setSnapshot(base);
    setRaftSettings(raft);
    // The store's effective settings (a line base forces the wall off).
    record.raft = { ...getRaftSettings() };
    const models = clearance ? [soupSource(fixN.meshes[src.scene], true)] : [];
    try {
        record.triangles = digest(flat(buildSupportAndRaftWorldTriangles(visible, undefined, shrink, models as any)));
    } catch (e) {
        record.error = e instanceof Error ? `${e.name}: ${e.message}` : String(e);
    }
    cases.push(record);
}
// Contact primitives with a tip penetration and scale.
const primitives: unknown[] = [];
for (const src of sources) {
    for (const key of ['trunks', 'branches', 'leaves', 'stumps', 'sticks', 'twigs']) {
        for (const e of Object.values(src.state[key] ?? {}) as any[]) {
            for (const field of ['contactCone', 'contactConeA', 'contactConeB', 'contactDiskA', 'contactDiskB']) {
                const c = e[field];
                if (!c || rand() > 0.02) continue;
                const disk = field.startsWith('contactDisk');
                const segments = pick([4, 8, 12]), penetration = pick([0, 0.05, range(0.01, 0.4)]), scale = pick([1, 0.75, range(0.4, 1)]);
                const out: any[] = [];
                (disk ? appendContactDiskPrimitive : appendContactConePrimitive)(out, c, segments, penetration, scale);
                primitives.push({ disk, contact: c, segments, penetration, scale, triangles: digest(flat(out)) });
            }
        }
    }
}
fs.unlinkSync('./src/features/slicing/_rasterOracle.ts');
process.stdout.write(JSON.stringify({ cases, primitives }));
}
main();
