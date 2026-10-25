// Parity oracle for block 4 of the native port: PlacementLogicV3 (the
// EscapeJointSearch walks, SmartPlacementV3) and buildTrunkData with a mesh,
// plus the helpers they bring (smartPlacementSearchUtils,
// nearestCandidateNodeKeys). Router calls are recorded in call order with the
// router counters after each, because the pooled distance field and its column
// maps make answers depend on what was asked before.
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_v3.json
//
// Run with DF_ORACLE_DETERMINISTIC_UUID=1 run_df_script.sh, so `uuid` is the
// counter in uuid_stub.mjs and entity ids compare exactly.
import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { __resetUuid } from 'uuid';
import { initializeBVH, accelerateGeometry } from './src/utils/bvh';
import { calculateSmartPlacementV3, getRouterStats, resetRouterStats } from './src/supports/PlacementLogicV3/SmartPlacementV3';
import { findEscapeJoint, findGridJoint, buildDirectionFan } from './src/supports/PlacementLogicV3/EscapeJointSearch';
import { buildNearestCandidateNodeKeys } from './src/supports/PlacementLogic/Grid/nearestCandidateNodeKeys';
import * as su from './src/supports/PlacementLogic/smartPlacementSearchUtils';
import { buildTrunkData } from './src/supports/SupportTypes/Trunk/trunkBuilder';
import { createDefaultSettings } from './src/supports/Settings/types';
import { setSettings, getSettings } from './src/supports/Settings/state';
import type { SDFCache } from './src/supports/PlacementLogic/Pathfinding/SDFCache';

const realLog = console.log;
console.log = () => {};
console.warn = () => {};
initializeBVH();

let seed = 0xb10c4;
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

// --- the analytic model DragonFruit's own tests search against ---
type Box = { min: [number, number, number]; max: [number, number, number] };
function signedDistanceToBox(p: { x: number; y: number; z: number }, box: Box): number {
    const qx = Math.max(box.min[0] - p.x, p.x - box.max[0]);
    const qy = Math.max(box.min[1] - p.y, p.y - box.max[1]);
    const qz = Math.max(box.min[2] - p.z, p.z - box.max[2]);
    return Math.hypot(Math.max(qx, 0), Math.max(qy, 0), Math.max(qz, 0)) + Math.min(Math.max(qx, qy, qz), 0);
}
function makeBoxSdf(boxes: Box[]): SDFCache {
    const distanceAt = (x: number, y: number, z: number): number => {
        let best = Infinity;
        for (const box of boxes) best = Math.min(best, signedDistanceToBox({ x, y, z }, box));
        return best;
    };
    return {
        cellSize: 0.5,
        distanceAt,
        segmentBlocked: (ax: number, ay: number, az: number, bx: number, by: number, bz: number, clearance: number) => {
            const len = Math.hypot(bx - ax, by - ay, bz - az);
            const steps = Math.max(1, Math.ceil(len / 0.25));
            for (let i = 0; i <= steps; i++) {
                const t = i / steps;
                if (distanceAt(ax + (bx - ax) * t, ay + (by - ay) * t, az + (bz - az) * t) < clearance) return true;
            }
            return false;
        },
    } as unknown as SDFCache;
}
/** A deterministic, sparse refusal, so the base gate is exercised. */
const baseFitsAt = (x: number, y: number) => (Math.floor(x * 7) + Math.floor(y * 3)) % 5 !== 0;
function randomBoxes(): Box[] {
    return Array.from({ length: 1 + Math.floor(rand() * 4) }, () => {
        const cx = range(-8, 8), cy = range(-8, 8), hx = range(0.5, 6), hy = range(0.5, 6);
        return { min: [cx - hx, cy - hy, 0], max: [cx + hx, cy + hy, range(2, 19)] } as Box;
    });
}
out.escape = Array.from({ length: 300 }, () => {
    const boxes = randomBoxes();
    const socket = { x: range(-3, 3), y: range(-3, 3), z: range(5, 24) };
    const rootTopZ = range(0, 3);
    const grid = rand() < 0.5;
    const directions = rand() < 0.1 ? [] : buildDirectionFan(rand() < 0.2 ? null : { x: range(-1, 1), y: range(-1, 1) }, pick([1, 4, 12]));
    const common = {
        clearanceMm: range(0.3, 1.5),
        maxLateralMm: pick([48, range(1, 30)]),
        leanFromVerticalDeg: pick([45, range(20, 60)]),
        minVerticalLegMm: pick([1, range(0, 3)]),
        baseFitsAt: rand() < 0.5 ? () => true : baseFitsAt,
    };
    const fits = common.baseFitsAt === baseFitsAt;
    const sdf = makeBoxSdf(boxes);
    if (grid) {
        const options = {
            ...common, spacingMm: pick([0, range(0.7, 6)]), maxNodeCount: pick([24, Math.floor(range(1, 60))]),
            preferredDirection: rand() < 0.2 ? null : { x: range(-1, 1), y: range(-1, 1) },
        };
        return { kind: 'grid', boxes, socket, rootTopZ, options: { ...options, baseFitsAt: fits }, result: findGridJoint(sdf, socket, rootTopZ, options) };
    }
    const options = { ...common, stepMm: pick([0.5, range(0.1, 2)]), directions };
    return { kind: 'escape', boxes, socket, rootTopZ, options: { ...options, baseFitsAt: fits }, result: findEscapeJoint(sdf, socket, rootTopZ, options) };
});

out.fans = Array.from({ length: 100 }, () => {
    const preferred = rand() < 0.2 ? null : { x: range(-1, 1), y: range(-1, 1) };
    const count = Math.floor(range(0, 20));
    return { preferred, count, result: buildDirectionFan(preferred, count) };
});

out.nodeKeys = Array.from({ length: 60 }, () => {
    const key = pick([`${Math.floor(range(-20, 20))},${Math.floor(range(-20, 20))}`, 'NaN,3', '4', '', ' 2 , -1 ']);
    const rings = Math.floor(range(0, 5));
    return { key, rings, result: buildNearestCandidateNodeKeys(key, rings) };
});

out.searchUtils = Array.from({ length: 400 }, () => {
    const a = { x: range(-5, 5), y: range(-5, 5), z: range(0, 20) };
    const b = rand() < 0.1 ? { ...a, z: a.z } : { x: range(-5, 5), y: range(-5, 5), z: range(0, 20) };
    const len = range(0, 12), base = range(5, 80), floor = rand() < 0.3 ? undefined : range(5, 60);
    return {
        a, b, len, base, floor,
        distanceXY: su.distanceXY(a, b),
        distance3D: su.distance3D(a, b),
        fromHorizontal: su.segmentAngleFromHorizontalDeg(a, b),
        fromVertical: su.segmentAngleFromVerticalDeg(a, b),
        departure: su.memberDepartureAngleFromVerticalDeg(a, b),
        spanLean: su.spanLeanFromVerticalDeg(a, b),
        lengthAware: su.getLengthAwareMaxAngleFromVerticalDeg(len, base, floor),
        satisfiesLengthAware: su.segmentSatisfiesLengthAwareMaxAngleFromVertical(a, b, base, floor),
        socketElbow: su.firstSegmentSatisfiesSocketElbowMaxAngle(a, b, base),
        chain: su.chainSatisfiesLengthAwareUpperSpanRule([a, b, { x: b.x, y: b.y, z: b.z - range(0, 5) }], base),
        positionKey: su.positionKey(a),
    };
});

// --- meshes for the router ---
function asMesh(geometry: THREE.BufferGeometry, matrix?: THREE.Matrix4) {
    const positions = Array.from(geometry.attributes.position.array as Float32Array);
    const index = geometry.index ? Array.from(geometry.index.array as ArrayLike<number>) : null;
    accelerateGeometry(geometry);
    const mesh = new THREE.Mesh(geometry, new THREE.MeshBasicMaterial());
    if (matrix) matrix.decompose(mesh.position, mesh.quaternion, mesh.scale);
    mesh.updateMatrixWorld(true);
    return { mesh, positions, index };
}
/** DragonFruit's jaw chip overhanging a body slab. */
function jawGeometry() {
    const body = new THREE.BoxGeometry(25, 20, 10);
    body.translate(-7.5, 0, 5);
    const jaw = new THREE.BoxGeometry(4, 4, 2);
    jaw.translate(3.5, 0, 17);
    return mergeGeometries([body, jaw])!;
}
function wallGeometry() {
    const geometry = new THREE.BoxGeometry(10, 20, 30);
    geometry.translate(5, 0, 15);
    return geometry;
}
/** A table: a slab on four legs, so tips under it have to route out. */
function tableGeometry() {
    const parts = [new THREE.BoxGeometry(20, 14, 1.5).translate(0, 0, 14)];
    for (const [x, y] of [[-9, -6], [9, -6], [-9, 6], [9, 6]]) parts.push(new THREE.BoxGeometry(1.5, 1.5, 13.5).translate(x, y, 6.75));
    // A crate under the slab, off centre, so straight columns are blocked.
    parts.push(new THREE.BoxGeometry(8, 6, 6).translate(-2, 1, 3));
    return mergeGeometries(parts)!;
}
const knotGeometry = () => new THREE.TorusKnotGeometry(8, 2.5, 96, 10).translate(0, 0, 14);
const moved = new THREE.Matrix4().compose(
    new THREE.Vector3(2, -1, 3), new THREE.Quaternion().setFromEuler(new THREE.Euler(0.2, -0.1, 0.5)), new THREE.Vector3(1, 1, 1),
);
const scenes = {
    knot: asMesh(knotGeometry()),
    jaw: asMesh(jawGeometry()),
    wall: asMesh(wallGeometry()),
    table: asMesh(tableGeometry()),
    knotMoved: asMesh(knotGeometry(), moved),
};
type SceneName = keyof typeof scenes;
out.meshes = Object.fromEntries(Object.entries(scenes).map(([name, s]) => [name, {
    positions: s.positions, index: s.index, matrix: s.mesh.matrixWorld.elements.slice(),
}]));

/** A surface point (in world space) and its face normal, mostly facing down. */
function surfacePoint(name: SceneName) {
    const { mesh, positions, index } = scenes[name];
    const triCount = (index ? index.length : positions.length / 3) / 3;
    const vertex = (i: number) => {
        const k = index ? index[i] : i;
        return new THREE.Vector3(positions[k * 3], positions[k * 3 + 1], positions[k * 3 + 2]).applyMatrix4(mesh.matrixWorld);
    };
    const wantDown = rand() < 0.8;
    for (let attempt = 0; ; attempt++) {
        const t = Math.floor(rand() * triCount) * 3;
        const a = vertex(t), b = vertex(t + 1), c = vertex(t + 2);
        const n = new THREE.Triangle(a, b, c).getNormal(new THREE.Vector3());
        if (wantDown && n.z > -0.2 && attempt < 200) continue;
        let u = rand(), v = rand();
        if (u + v > 1) { u = 1 - u; v = 1 - v; }
        const p = a.clone().addScaledVector(b.clone().sub(a), u).addScaledVector(c.clone().sub(a), v);
        return { pos: v3(p), normal: v3(n) };
    }
}

function routerSettings() {
    const s: any = createDefaultSettings();
    s.tip.coneAngleMode = pick(['normal', 'locked', 'adaptive']);
    if (rand() < 0.5) s.tip.adaptiveConeAngleOffsetDeg = range(0, 60);
    s.tip.contactDiameterMm = range(0.2, 0.8);
    s.tip.bodyDiameterMm = range(0.6, 1.8);
    s.tip.lengthMm = range(0.8, 4);
    s.shaft.diameterMm = range(0.4, 2.2);
    s.roots.diameterMm = range(1, 8);
    s.roots.diskHeightMm = range(0, 1);
    s.roots.coneHeightMm = range(0, 3);
    s.grid.enabled = rand() < 0.5;
    s.grid.spacingMm = pick([2, range(1, 6)]);
    s.grid.minRoutedTrunkAngleDeg = pick([s.grid.minRoutedTrunkAngleDeg, range(10, 85)]);
    setSettings(s);
    return structuredClone(getSettings());
}
function tipProfileFor(settings: any) {
    return {
        type: 'disk' as const, contactDiameterMm: settings.tip.contactDiameterMm, bodyDiameterMm: settings.tip.bodyDiameterMm,
        lengthMm: settings.tip.lengthMm, penetrationMm: settings.tip.penetrationMm,
        diskThicknessMm: settings.tip.diskThicknessMm ?? 0.1, maxStandoffMm: settings.tip.maxStandoffMm ?? 1.5,
        standoffAngleThreshold: settings.tip.standoffAngleThreshold ?? Math.PI / 4,
    };
}

// DragonFruit's own placement-contract cases, verbatim.
const TIP_PROFILE = {
    type: 'disk' as const, contactDiameterMm: 0.4, bodyDiameterMm: 1.2, lengthMm: 1.2, penetrationMm: 0.05,
    diskThicknessMm: 0.1, maxStandoffMm: 0.35, standoffAngleThreshold: Math.PI / 4,
};
const WALL_TIP_PROFILE = { ...TIP_PROFILE, bodyDiameterMm: 1.6, lengthMm: 2.5 };

const ops: unknown[] = [];
resetRouterStats();
function record(op: Record<string, unknown>) {
    ops.push({ ...op, stats: structuredClone(getRouterStats()) });
}
function contract(name: string, scene: SceneName, settingsPatch: (s: any) => void, input: any) {
    const s: any = createDefaultSettings();
    settingsPatch(s);
    setSettings(s);
    const settings = structuredClone(getSettings());
    const result = calculateSmartPlacementV3({ ...input, mesh: scenes[scene].mesh });
    record({ op: 'v3', name, scene, matrix: scenes[scene].mesh.matrixWorld.elements.slice(), settings, input, result });
}
const jawTip = { tipPos: { x: 3.5, y: 0, z: 16 }, tipNormal: { x: 0, y: 0, z: -1 }, tipProfile: TIP_PROFILE, modelId: 'model-v3', rootsTopZ: 2 };
contract('jaw', 'jaw', (s) => { s.grid.enabled = false; }, jawTip);
contract('jawGrid', 'jaw', (s) => { s.grid.enabled = true; s.grid.spacingMm = 2; }, jawTip);
contract('wall', 'wall', (s) => { s.grid.enabled = false; s.tip.coneAngleMode = 'normal'; }, {
    tipPos: { x: 0, y: 0, z: 15 }, tipNormal: { x: -1, y: 0, z: 0 }, tipProfile: WALL_TIP_PROFILE, modelId: 'model-wall', rootsTopZ: 2,
});

const sceneNames = Object.keys(scenes) as SceneName[];
for (let i = 0; i < 700; i++) {
    const scene = pick(sceneNames);
    // The moved knot drifts mid-run: its field must notice and start over.
    if (scene === 'knotMoved' && rand() < 0.05) {
        const mesh = scenes.knotMoved.mesh;
        mesh.position.x += range(-0.5, 0.5);
        mesh.rotation.z += range(-0.05, 0.05);
        mesh.updateMatrixWorld(true);
    }
    const matrix = scenes[scene].mesh.matrixWorld.elements.slice();
    const settings = routerSettings();
    const tip = rand() < 0.9 ? surfacePoint(scene) : { pos: { x: range(-12, 12), y: range(-12, 12), z: range(1, 26) }, normal: unit() };
    if (rand() < 0.6) {
        const input = {
            tipPos: tip.pos, tipNormal: tip.normal, tipProfile: tipProfileFor(settings), rootsTopZ: range(0, 3),
            isSmallIsland: rand() < 0.15 ? true : undefined, modelId: 'm', isPreview: rand() < 0.3 ? true : undefined,
        };
        if (input.isSmallIsland === undefined) delete input.isSmallIsland;
        if (input.isPreview === undefined) delete input.isPreview;
        const ignoreGrid = rand() < 0.15;
        const result = calculateSmartPlacementV3({ ...input, mesh: scenes[scene].mesh }, ignoreGrid ? { ignoreGrid: true } : undefined);
        record({ op: 'v3', scene, matrix, settings, input, ignoreGrid, result });
    } else {
        const overrides: any = rand() < 0.5 ? undefined : {
            rootsDiameterMm: rand() < 0.5 ? undefined : range(1, 4),
            rootsDiskHeightMm: rand() < 0.5 ? undefined : range(0, 1),
            rootsConeHeightMm: rand() < 0.5 ? undefined : range(0, 2),
            shaftDiameterMm: rand() < 0.5 ? undefined : range(0.5, 2),
            tipContactDiameterMm: rand() < 0.5 ? undefined : range(0.1, 0.6),
            tipBodyDiameterMm: rand() < 0.5 ? undefined : range(0.5, 2),
            tipLengthMm: rand() < 0.5 ? undefined : range(1, 4),
            tipDiskLengthOverrideMm: rand() < 0.7 ? undefined : range(0, 0.5),
        };
        if (overrides) for (const k of Object.keys(overrides)) if (overrides[k] === undefined) delete overrides[k];
        const input: any = { tipPos: tip.pos, tipNormal: tip.normal, modelId: 'm' };
        if (overrides) input.overrides = overrides;
        if (rand() < 0.15) input.isSmallIsland = true;
        if (rand() < 0.3) input.isPreview = true;
        if (rand() < 0.15) input.ignoreGrid = true;
        __resetUuid();
        const result = buildTrunkData({ ...input, mesh: scenes[scene].mesh });
        record({ op: 'buildTrunkData', scene, matrix, settings, input, result });
    }
}
out.ops = ops;

process.stdout.write(JSON.stringify(out));
realLog('');
