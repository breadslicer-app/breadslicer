// Parity oracle for block 5 of the native port: the Twig, Stick, Stump and
// Kickstand builders, the registered contact bridge builders (verticality
// gates) and the stump's contact override (body/joint/root collision), the
// twig taper, and the brace's span diameters. Mesh checks are recorded in call
// order, because the pooled distance field makes answers depend on what was
// asked before.
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_builders5.json
//
// Run with DF_ORACLE_DETERMINISTIC_UUID=1 run_df_script.sh, so `uuid` is the
// counter in uuid_stub.mjs and entity ids compare exactly.
import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { __resetUuid } from 'uuid';
// Every type's registration runs from the barrel `state.ts` loads, so enter the
// graph there, as DragonFruit's own focused tests do.
import './src/supports/state';
import { initializeBVH, accelerateGeometry } from './src/utils/bvh';
import { buildContactBridge, buildContactOverride } from './src/supports/supportTypeRegistry';
import { buildTwig } from './src/supports/SupportTypes/Twig/twigBuilder';
import { buildStick } from './src/supports/SupportTypes/Stick/stickBuilder';
import { buildStumpData } from './src/supports/SupportTypes/Stump/stumpBuilder';
import { buildKickstandData, toKickstandPreviewData } from './src/supports/SupportTypes/Kickstand/kickstandBuilder';
import { resolveKickstandLayout } from './src/supports/SupportTypes/Kickstand/kickstandSettings';
import { resolveTwigDiameterAtSegmentT, resolveTwigSegmentDiameters, buildTwigTaperLayout } from './src/supports/SupportTypes/Twig/twigTaper';
import { braceSpanDiameters } from './src/supports/SupportTypes/Brace/braceSpanDiameters';
import { createDefaultSettings } from './src/supports/Settings/types';
import { setSettings, getSettings } from './src/supports/Settings/state';

const realLog = console.log;
console.log = () => {};
console.warn = () => {};
initializeBVH();

let seed = 0xb10c5;
function rand(): number {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const range = (lo: number, hi: number) => lo + (hi - lo) * rand();
const pick = <T>(xs: readonly T[]): T => xs[Math.floor(rand() * xs.length)];
const v3 = (v: { x: number; y: number; z: number }) => ({ x: v.x, y: v.y, z: v.z });
const unit = () => {
    const p = new THREE.Vector3(range(-1, 1), range(-1, 1), range(-1, 1));
    if (p.lengthSq() < 1e-6) p.set(0, 0, 1);
    p.normalize();
    return v3(p);
};
const out: Record<string, unknown> = {};

// --- meshes ---
function asMesh(geometry: THREE.BufferGeometry, matrix?: THREE.Matrix4) {
    const positions = Array.from(geometry.attributes.position.array as Float32Array);
    const index = geometry.index ? Array.from(geometry.index.array as ArrayLike<number>) : null;
    accelerateGeometry(geometry);
    const mesh = new THREE.Mesh(geometry, new THREE.MeshBasicMaterial());
    if (matrix) matrix.decompose(mesh.position, mesh.quaternion, mesh.scale);
    mesh.updateMatrixWorld(true);
    return { mesh, positions, index };
}
/** A ceiling over a floor slab, with a post between: gaps for bridges to span. */
function gapGeometry() {
    return mergeGeometries([
        new THREE.BoxGeometry(24, 18, 2).translate(0, 0, 1),
        new THREE.BoxGeometry(24, 18, 2).translate(0, 0, 9),
        new THREE.BoxGeometry(2, 2, 6).translate(4, -2, 5),
        new THREE.BoxGeometry(6, 3, 1).translate(-5, 4, 2.5),
    ])!;
}
/** A low overhang lip over the plate with a short column under it: stumps. */
function lowGeometry() {
    return mergeGeometries([
        new THREE.BoxGeometry(12, 10, 1).translate(0, 0, 2),
        new THREE.BoxGeometry(1, 1, 1.5).translate(2, 1, 0.75),
        new THREE.BoxGeometry(0.6, 6, 1.2).translate(-3, 0, 0.6),
        // A higher lip (underside z = 3.5) with a column beside it: longer cones.
        new THREE.BoxGeometry(6, 6, 1).translate(10, 0, 4),
        new THREE.BoxGeometry(0.8, 0.8, 3.5).translate(10, 1.5, 1.75),
    ])!;
}
const knotGeometry = () => new THREE.TorusKnotGeometry(8, 2.5, 96, 10).translate(0, 0, 14);
const moved = new THREE.Matrix4().compose(
    new THREE.Vector3(1.5, -2, 0.5), new THREE.Quaternion().setFromEuler(new THREE.Euler(0.15, -0.1, 0.4)), new THREE.Vector3(1, 1, 1),
);
const scenes = {
    knot: asMesh(knotGeometry()),
    gap: asMesh(gapGeometry()),
    gapMoved: asMesh(gapGeometry(), moved),
    low: asMesh(lowGeometry()),
};
type SceneName = keyof typeof scenes;
out.meshes = Object.fromEntries(Object.entries(scenes).map(([name, s]) => [name, {
    positions: s.positions, index: s.index, matrix: s.mesh.matrixWorld.elements.slice(),
}]));

/** A point on the scene's surface (world space) and its face normal. */
function surfacePoint(name: SceneName, want?: (n: THREE.Vector3) => boolean) {
    const { mesh, positions, index } = scenes[name];
    const triCount = (index ? index.length : positions.length / 3) / 3;
    const vertex = (i: number) => {
        const k = index ? index[i] : i;
        return new THREE.Vector3(positions[k * 3], positions[k * 3 + 1], positions[k * 3 + 2]).applyMatrix4(mesh.matrixWorld);
    };
    for (let attempt = 0; ; attempt++) {
        const t = Math.floor(rand() * triCount) * 3;
        const a = vertex(t), b = vertex(t + 1), c = vertex(t + 2);
        const n = new THREE.Triangle(a, b, c).getNormal(new THREE.Vector3());
        if (want && !want(n) && attempt < 300) continue;
        let u = rand(), v = rand();
        if (u + v > 1) { u = 1 - u; v = 1 - v; }
        const p = a.clone().addScaledVector(b.clone().sub(a), u).addScaledVector(c.clone().sub(a), v);
        return { pos: v3(p), normal: v3(n) };
    }
}
/** A pair of contacts to bridge: a ceiling/floor pair, nearby surface points, or anything. */
function contactPair(scene: SceneName) {
    const r = rand();
    if (r < 0.45) {
        const a = surfacePoint(scene, (n) => n.z < -0.3);
        const b = surfacePoint(scene, (n) => n.z > 0.3);
        return { a, b };
    }
    if (r < 0.8) {
        const a = surfacePoint(scene);
        const b = surfacePoint(scene);
        return { a, b };
    }
    const a = { pos: { x: range(-10, 10), y: range(-8, 8), z: range(0, 20) }, normal: unit() };
    const b = rand() < 0.1
        ? { pos: { ...a.pos }, normal: unit() }
        : { pos: { x: a.pos.x + range(-4, 4), y: a.pos.y + range(-4, 4), z: a.pos.z + range(-8, 8) }, normal: unit() };
    return { a, b };
}

function randomSettings() {
    const s: any = createDefaultSettings();
    s.tip.coneAngleMode = pick(['normal', 'locked', 'adaptive', undefined]);
    if (s.tip.coneAngleMode === undefined) delete s.tip.coneAngleMode;
    if (rand() < 0.5) s.tip.adaptiveConeAngleOffsetDeg = range(-10, 90);
    s.tip.contactDiameterMm = range(0.1, 1.2);
    s.tip.bodyDiameterMm = range(0.4, 2.5);
    s.tip.lengthMm = range(0.5, 6);
    s.tip.penetrationMm = range(0, 0.3);
    if (rand() < 0.3) delete s.tip.diskThicknessMm; else s.tip.diskThicknessMm = range(0.02, 0.4);
    s.tip.maxStandoffMm = range(0.1, 2);
    if (rand() < 0.5) s.tip.standoffAngleThreshold = range(0.05, 1.4);
    s.shaft.diameterMm = range(0.3, 3);
    s.roots.diameterMm = range(-1, 10);
    s.roots.diskHeightMm = range(-0.5, 2);
    s.roots.coneHeightMm = range(-0.5, 4);
    setSettings(s);
    return structuredClone(getSettings());
}

const sceneNames = Object.keys(scenes) as SceneName[];
const ops: unknown[] = [];
const hostKinds = ['trunk', 'branch', 'twig', 'stick', 'brace', 'stump', 'kickstand', 'leaf'] as const;

for (let i = 0; i < 900; i++) {
    const scene = pick(sceneNames);
    const settings = randomSettings();
    const withMesh = rand() < 0.75;
    const mesh = withMesh ? scenes[scene].mesh : undefined;
    const kind = pick(['twig', 'stick', 'bridge', 'stump', 'stumpOverride', 'kickstand'] as const);
    __resetUuid();
    if (kind === 'twig' || kind === 'stick' || kind === 'bridge') {
        const { a, b } = contactPair(scene);
        const input: any = { modelId: 'm', aPos: a.pos, aNormal: a.normal, bPos: b.pos, bNormal: b.normal };
        if (rand() < 0.4) input.tipContactDiameterMm = range(0.1, 1);
        if (kind !== 'twig' && rand() < 0.4) input.shaftDiameterMm = range(0.3, 2.5);
        if (kind === 'twig') {
            ops.push({ op: kind, scene, withMesh, settings, input, result: buildTwig({ ...input, mesh }) });
        } else if (kind === 'stick') {
            ops.push({ op: kind, scene, withMesh, settings, input, result: buildStick({ ...input, mesh }) });
        } else {
            const typeId = pick(['twig', 'stick', 'trunk'] as const);
            if (rand() < 0.3) input.manual = true;
            const result = buildContactBridge(typeId, { ...input, mesh });
            ops.push({ op: 'bridge', typeId, scene, withMesh, settings, input, result });
        }
    } else if (kind === 'stump' || kind === 'stumpOverride') {
        const tip = rand() < 0.85
            ? surfacePoint(scene, (n) => n.z < -0.2)
            : { pos: { x: range(-10, 10), y: range(-8, 8), z: range(0.5, 6) }, normal: unit() };
        const input = { tipPos: tip.pos, tipNormal: tip.normal, modelId: 'm' };
        if (kind === 'stump') {
            ops.push({ op: kind, scene, withMesh, settings, input, result: buildStumpData({ ...input, mesh }) });
        } else {
            const result = buildContactOverride('stump')!({ ...input, mesh });
            ops.push({ op: kind, scene, withMesh, settings, input, result });
        }
    } else {
        const top = surfacePoint(scene);
        const host: any = {
            segmentId: 'seg-1', supportKind: pick(hostKinds), t: range(-0.3, 1.3),
            pos: rand() < 0.8 ? { x: top.pos.x, y: top.pos.y, z: top.pos.z - range(0, 3) } : { x: range(-10, 10), y: range(-8, 8), z: range(-1, 20) },
            diameterMm: range(-0.2, 2.5),
        };
        if (rand() < 0.4) host.minT = range(0, 0.8);
        const input: any = {
            modelId: 'm',
            rootPos: { x: host.pos.x + range(-6, 6), y: host.pos.y + range(-6, 6), z: pick([0, range(-0.5, 2)]) },
            host,
        };
        if (rand() < 0.5) {
            const o: any = {};
            for (const k of ['firstJointHeightRatio', 'secondJointHeightRatio', 'minJointSpacingMm', 'minTerminalClearanceMm']) {
                if (rand() < 0.5) o[k] = range(-0.5, 6);
            }
            input.layoutOverrides = o;
        }
        if (rand() < 0.3) input.authoredColumn = { topPos: { x: input.rootPos.x + range(-1, 1), y: input.rootPos.y + range(-1, 1), z: range(-1, host.pos.z + 1) } };
        if (rand() < 0.5) input.maxShaftAngleDeg = range(10, 90);
        let result: unknown;
        let error: string | undefined;
        try {
            const built = buildKickstandData({ ...input, collisionMesh: mesh });
            result = { build: built, preview: toKickstandPreviewData(built) };
        } catch (e) {
            error = (e as Error).message;
        }
        ops.push({ op: 'kickstand', scene, withMesh, settings, input, result, error });
    }
}

// --- sweeps: thresholds the random draw rarely reaches ---
const sweepOps: unknown[] = [];
function sweep(op: string, scene: SceneName, settings: unknown, input: any, run: () => unknown, extra: Record<string, unknown> = {}) {
    __resetUuid();
    let result: unknown;
    let error: string | undefined;
    try { result = run(); } catch (e) { error = (e as Error).message; }
    sweepOps.push({ op, scene, withMesh: true, settings, input, result, error, ...extra });
}
// Post in the gap scene: x ∈ [3, 5], y ∈ [-3, -1], z ∈ [2, 8]. Ceiling underside z = 8, floor top z = 2.
for (let i = 0; i < 160; i++) {
    const settings = randomSettings();
    const d = range(-0.2, 1.6);
    const y = range(-3.2, -0.8);
    const side = rand() < 0.5;
    const x = side ? 5 + d : 3 - d;
    const a = { pos: { x, y, z: 8 }, normal: { x: 0, y: 0, z: -1 } };
    const b = { pos: { x: x + range(-0.4, 0.4), y: y + range(-0.4, 0.4), z: 2 }, normal: { x: 0, y: 0, z: 1 } };
    const input: any = { modelId: 'm', aPos: a.pos, aNormal: a.normal, bPos: b.pos, bNormal: b.normal };
    if (rand() < 0.5) input.tipContactDiameterMm = range(0.1, 1.2);
    if (rand() < 0.5) input.shaftDiameterMm = range(0.3, 2.5);
    if (i % 2 === 0) sweep('twig', 'gap', settings, input, () => buildTwig({ ...input, mesh: scenes.gap.mesh }));
    else sweep('stick', 'gap', settings, input, () => buildStick({ ...input, mesh: scenes.gap.mesh }));
}
// Sidewall contacts on the post, bridged across to the opposite wall or a short hop.
for (let i = 0; i < 120; i++) {
    const settings = randomSettings();
    const z = range(2.5, 7.5), y = range(-2.9, -1.1);
    const a = { pos: { x: 5, y, z }, normal: { x: 1, y: 0, z: 0 } };
    const b = rand() < 0.5
        ? { pos: { x: 5 + range(0.2, 4), y: y + range(-1, 1), z: pick([2, 8]) }, normal: { x: 0, y: 0, z: z > 5 ? 1 : -1 } }
        : { pos: { x: 3, y: y + range(-0.5, 0.5), z: z + range(-2, 2) }, normal: { x: -1, y: 0, z: 0 } };
    const input: any = { modelId: 'm', aPos: a.pos, aNormal: a.normal, bPos: b.pos, bNormal: b.normal };
    if (rand() < 0.5) input.shaftDiameterMm = range(0.3, 2);
    const kind = pick(['twig', 'stick', 'bridge'] as const);
    if (kind === 'twig') sweep(kind, 'gap', settings, input, () => buildTwig({ ...input, mesh: scenes.gap.mesh }));
    else if (kind === 'stick') sweep(kind, 'gap', settings, input, () => buildStick({ ...input, mesh: scenes.gap.mesh }));
    else {
        const typeId = pick(['twig', 'stick'] as const);
        sweep('bridge', 'gap', settings, input, () => buildContactBridge(typeId, { ...input, mesh: scenes.gap.mesh }), { typeId });
    }
}
// Stick end ordering: ties on z, then on y.
for (let i = 0; i < 40; i++) {
    const settings = randomSettings();
    const a = { x: range(-5, 5), y: range(-5, 5), z: range(3, 7) };
    const b = { x: a.x + range(-3, 3), y: i % 2 ? a.y : a.y + range(-3, 3), z: a.z };
    const input: any = { modelId: 'm', aPos: a, aNormal: unit(), bPos: b, bNormal: unit() };
    sweep('stick', 'gap', settings, input, () => buildStick({ ...input }));
    (sweepOps[sweepOps.length - 1] as any).withMesh = false;
}
// Stumps under the low lip (underside z = 1.5), walking toward the column at
// x ∈ [1.5, 2.5], y ∈ [0.5, 1.5] and the fin at x ∈ [-3.3, -2.7].
for (let i = 0; i < 200; i++) {
    const settings = randomSettings();
    const target = pick([[2, 1], [-3, range(-3, 3)]]);
    const tipPos = { x: target[0] + pick([-1, 1]) * range(0.3, 2.2), y: target[1] + range(-1.5, 1.5), z: 1.5 };
    const tilt = rand() < 0.5 ? 0 : range(-0.6, 0.6);
    const tipNormal = v3(new THREE.Vector3(tilt, range(-0.3, 0.3), -1).normalize());
    const input = { tipPos, tipNormal, modelId: 'm' };
    if (i % 2 === 0) sweep('stumpOverride', 'low', settings, input, () => buildContactOverride('stump')!({ ...input, mesh: scenes.low.mesh }));
    else sweep('stump', 'low', settings, input, () => buildStumpData({ ...input, mesh: scenes.low.mesh }));
}
// Kickstands whose column passes the post at a walking distance, ending at a
// host near the ceiling.
for (let i = 0; i < 160; i++) {
    const settings = randomSettings();
    const d = range(-0.3, 2);
    const host: any = { segmentId: 'seg-1', supportKind: 'trunk', t: range(0, 1), pos: { x: 5 + d + range(0, 0.6), y: range(-3, -1), z: range(5, 7.8) }, diameterMm: range(0.3, 2.5) };
    const input: any = { modelId: 'm', rootPos: { x: 5 + d, y: host.pos.y + range(-0.5, 0.5), z: 2 }, host };
    if (rand() < 0.3) input.authoredColumn = { topPos: { x: input.rootPos.x, y: input.rootPos.y, z: host.pos.z - range(0.5, 2) } };
    sweep('kickstand', 'gap', settings, input, () => {
        const built = buildKickstandData({ ...input, collisionMesh: scenes.gap.mesh });
        return { build: built, preview: toKickstandPreviewData(built) };
    });
}

// Stumps under the higher lip, walking toward its column (x ∈ [9.6, 10.4], y ∈ [1.1, 1.9]).
for (let i = 0; i < 300; i++) {
    const settings = randomSettings();
    const angle = range(0, Math.PI * 2);
    const d = 0.4 + range(0, 1.4);
    const tipPos = { x: 10 + Math.cos(angle) * d, y: 1.5 + Math.sin(angle) * d, z: 3.5 };
    const toward = new THREE.Vector3(10 - tipPos.x, 1.5 - tipPos.y, 0).normalize().multiplyScalar(pick([0, 0.3, -0.3, 0.6]));
    const tipNormal = v3(new THREE.Vector3(toward.x, toward.y, -1).normalize());
    const input = { tipPos, tipNormal, modelId: 'm' };
    sweep('stumpOverride', 'low', settings, input, () => buildContactOverride('stump')!({ ...input, mesh: scenes.low.mesh }));
}
// Short-span sticks beside the post, rising off the floor (z = 2).
for (let i = 0; i < 160; i++) {
    const settings = randomSettings();
    const d = range(-0.1, 1.4);
    const x = 5 + d, y = range(-3.2, -0.8);
    const a = { pos: { x, y, z: 2 }, normal: { x: 0, y: 0, z: 1 } };
    const b = { pos: { x: x + range(-0.3, 0.3), y: y + range(-0.3, 0.3), z: 2 + range(0.4, 3.5) }, normal: { x: 0, y: 0, z: -1 } };
    const input: any = { modelId: 'm', aPos: a.pos, aNormal: a.normal, bPos: b.pos, bNormal: b.normal, shaftDiameterMm: range(0.3, 2.5) };
    sweep('stick', 'gap', settings, input, () => buildStick({ ...input, mesh: scenes.gap.mesh }));
}
out.ops = [...ops, ...sweepOps];

out.layouts = Array.from({ length: 200 }, () => {
    const o: any = {};
    for (const k of ['firstJointHeightRatio', 'secondJointHeightRatio', 'minJointSpacingMm', 'minTerminalClearanceMm']) {
        if (rand() < 0.6) o[k] = range(-1, 7);
    }
    const overrides = rand() < 0.1 ? undefined : o;
    return { overrides, result: resolveKickstandLayout(overrides) };
});

// The taper over multi-segment twigs with uneven ends (the builder's own twigs
// are cylinders).
out.tapers = Array.from({ length: 150 }, (_, i) => {
    const n = Math.floor(range(1, 5));
    const pts = Array.from({ length: n + 1 }, () => ({ x: range(-5, 5), y: range(-5, 5), z: range(0, 10) }));
    if (i % 10 === 0) for (const p of pts) Object.assign(p, pts[0]);
    if (i % 10 === 1) for (const [k, p] of pts.entries()) Object.assign(p, { x: pts[0].x + k * pick([1e-10, 3e-9, 2e-8]), y: pts[0].y, z: pts[0].z });
    const segments = Array.from({ length: n }, (_, k) => {
        const seg: any = { id: `s${k}`, diameter: 1 };
        if (rand() < 0.9) seg.bottomJoint = { id: `j${k}`, pos: pts[k], diameter: 1 };
        if (rand() < 0.9) seg.topJoint = { id: `j${k + 1}`, pos: pts[k + 1], diameter: 1 };
        return seg;
    });
    const disk = (d: number) => ({ id: 'd', pos: pts[0], surfaceNormal: { x: 0, y: 0, z: 1 }, coneAxis: { x: 0, y: 0, z: 1 }, profile: { type: 'disk' }, contactDiameterMm: d });
    const twig: any = { id: 't', modelId: 'm', segments, contactDiskA: disk(range(0, 2)), contactDiskB: disk(range(0, 2)) };
    const queries = Array.from({ length: 4 }, () => {
        const segmentId = pick([...segments.map((s) => s.id), 'nope']);
        const t = range(-0.5, 1.5);
        return { segmentId, t, result: resolveTwigDiameterAtSegmentT(twig, segmentId, t) };
    });
    return {
        twig, queries,
        layout: buildTwigTaperLayout(twig),
        diameters: Object.fromEntries(resolveTwigSegmentDiameters(twig)),
    };
});

out.braceSpans = Array.from({ length: 200 }, () => {
    const profile = rand() < 0.1 ? undefined : { diameter: range(-0.5, 5) };
    const knot = () => (rand() < 0.15 ? undefined : rand() < 0.15 ? {} : { diameter: range(-0.5, 3.5) });
    const start = knot();
    const end = rand() < 0.4 && start && (start as any).diameter !== undefined
        ? { diameter: (start as any).diameter + pick([5e-5, 2e-4, 5e-3, -3e-3]) }
        : knot();
    return { profile, start, end, result: braceSpanDiameters({ profile } as any, start as any, end as any) };
});

process.stdout.write(JSON.stringify(out));
realLog('');
