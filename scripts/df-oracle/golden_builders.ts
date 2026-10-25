// Parity oracle for block 3 of the native port: three-mesh-bvh raycasts,
// CollisionUtils, ConeAxisPolicy, Settings (normalization, hex codec),
// StandardPlacement, gridMath, and the Trunk, Branch and Leaf builders.
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_builders.json
//
// Run with DF_ORACLE_DETERMINISTIC_UUID=1 run_df_script.sh, so `uuid` is the
// counter in uuid_stub.mjs and entity ids compare exactly.
import * as THREE from 'three';
import { initializeBVH, accelerateGeometry } from './src/utils/bvh';
import { __resetUuid } from 'uuid';
import {
    resolveConeAxisPolicy, clampConeAxisDeviationFromSurfaceNormal, coneAxisLeanFromVerticalDeg,
    isSideWallContact, coneKeepsContactHighest,
} from './src/supports/PlacementLogic/ConeAxisPolicy';
import { checkShaftCollision, checkShortBridgeCollision, checkBezierCollision } from './src/supports/PlacementLogic/CollisionUtils';
import { createDefaultSettings } from './src/supports/Settings/types';
import { setSettings, getSettings } from './src/supports/Settings/state';
import { encodeSupportSettingsHex, decodeSupportSettingsHex } from './src/supports/Settings/supportSettingsCodec';
import { calculateStandardPlacement } from './src/supports/PlacementLogic/StandardPlacement';
import { snapToGridIndex, gridNodeKeyFromXY, gridSnappedXYFromKey } from './src/supports/PlacementLogic/Grid/gridMath';
import { buildTrunkData, buildTrunkDataFromPlacement } from './src/supports/SupportTypes/Trunk/trunkBuilder';
import { buildBranchData, remapBranchGeometryIds } from './src/supports/SupportTypes/Branch/branchBuilder';
import { buildLeafData } from './src/supports/SupportTypes/Leaf/leafBuilder';

const realLog = console.log;
console.log = () => {};
console.warn = () => {};
initializeBVH();

let seed = 0xb10c3;
function rand(): number {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const range = (lo: number, hi: number) => lo + (hi - lo) * rand();
const pick = <T>(xs: T[]): T => xs[Math.floor(rand() * xs.length)];
const unit = () => {
    const p = new THREE.Vector3(range(-1, 1), range(-1, 1), range(-1, 1));
    if (p.lengthSq() < 1e-6) p.set(0, 0, 1);
    p.normalize();
    return { x: p.x, y: p.y, z: p.z };
};
const v3 = (v: { x: number; y: number; z: number }) => ({ x: v.x, y: v.y, z: v.z });

/** A position-only, non-indexed geometry, as STLLoader produces. */
function soup(source: THREE.BufferGeometry): THREE.BufferGeometry {
    const flat = source.index ? source.toNonIndexed() : source;
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.Float32BufferAttribute(Array.from(flat.attributes.position.array), 3));
    return g;
}

const knot = soup(new THREE.TorusKnotGeometry(8, 2.5, 96, 10));
knot.translate(0, 0, 14);
const knotPositions = Array.from(knot.attributes.position.array as Float32Array);
accelerateGeometry(knot);
const knotMesh = new THREE.Mesh(knot);
knotMesh.updateMatrixWorld(true);
const box = new THREE.Box3().setFromBufferAttribute(knot.attributes.position as THREE.BufferAttribute);
const vin = (pad: number) => ({
    x: range(box.min.x - pad, box.max.x + pad), y: range(box.min.y - pad, box.max.y + pad), z: range(box.min.z - pad, box.max.z + pad),
});

// A point on the knot's surface and its face normal (before the BVH reorders).
const triCount = knotPositions.length / 9;
function surfacePoint() {
    const t = Math.floor(rand() * triCount) * 9;
    const a = new THREE.Vector3(knotPositions[t], knotPositions[t + 1], knotPositions[t + 2]);
    const b = new THREE.Vector3(knotPositions[t + 3], knotPositions[t + 4], knotPositions[t + 5]);
    const c = new THREE.Vector3(knotPositions[t + 6], knotPositions[t + 7], knotPositions[t + 8]);
    let u = rand(), w = rand();
    if (u + w > 1) { u = 1 - u; w = 1 - w; }
    const p = a.clone().add(b.clone().sub(a).multiplyScalar(u)).add(c.clone().sub(a).multiplyScalar(w));
    const n = new THREE.Triangle(a, b, c).getNormal(new THREE.Vector3());
    return { pos: v3(p), normal: v3(n) };
}

// A second, moved copy for raycasts under a world matrix.
const movedGeom = soup(new THREE.TorusKnotGeometry(8, 2.5, 96, 10));
const movedPositions = Array.from(movedGeom.attributes.position.array as Float32Array);
accelerateGeometry(movedGeom);
const movedMesh = new THREE.Mesh(movedGeom, new THREE.MeshBasicMaterial({ side: THREE.DoubleSide }));
movedMesh.matrixAutoUpdate = false;
movedMesh.matrix.compose(
    new THREE.Vector3(3, -2, 5), new THREE.Quaternion().setFromEuler(new THREE.Euler(0.3, -0.2, 0.7)), new THREE.Vector3(1.25, 0.9, 1.1),
);
movedMesh.matrixWorld.copy(movedMesh.matrix);

const out: Record<string, unknown> = {
    dragonfruit: 'f331fc555abb31c4bcb28d1a358edb887f0afe07',
    knotPositions, movedPositions, movedMatrix: Array.from(movedMesh.matrixWorld.elements),
};
const N = 160;
const modes = ['normal', 'locked', 'adaptive'] as const;

// --- cone axis policy ---
out.coneAxis = Array.from({ length: N * 2 }, () => {
    const n = rand() < 0.08 ? { x: 0, y: 0, z: pick([1, -1, 0]) } : unit();
    const mode = pick([...modes]);
    const offset = rand() < 0.2 ? undefined : range(-10, 100);
    const desired = unit();
    const maxDev = range(0, 60);
    const socket = vin(2);
    const tip = vin(2);
    const body = range(0.2, 2);
    return {
        n, mode, offset, desired, maxDev, socket, tip, body,
        policy: resolveConeAxisPolicy({ surfaceNormal: n, coneAngleMode: mode, adaptiveConeAngleOffsetDeg: offset }),
        clamp: clampConeAxisDeviationFromSurfaceNormal(n, desired, maxDev),
        lean: coneAxisLeanFromVerticalDeg(n, mode, offset),
        sideWall: isSideWallContact(n, mode, offset),
        highest: coneKeepsContactHighest({ coneAxis: desired, socketPos: socket, tipPos: tip, bodyDiameterMm: body }),
    };
});

// --- raycasts and collision ---
const raycaster = new THREE.Raycaster();
out.raycasts = Array.from({ length: N }, () => {
    const origin = vin(6);
    const dir = unit();
    const side = pick([THREE.FrontSide, THREE.BackSide, THREE.DoubleSide]);
    const near = rand() < 0.5 ? 0 : range(0, 5);
    const far = rand() < 0.3 ? Infinity : range(5, 60);
    const firstHitOnly = rand() < 0.5;
    (movedMesh.material as THREE.MeshBasicMaterial).side = side;
    raycaster.set(new THREE.Vector3(origin.x, origin.y, origin.z), new THREE.Vector3(dir.x, dir.y, dir.z));
    raycaster.near = near;
    raycaster.far = far;
    (raycaster as any).firstHitOnly = firstHitOnly;
    const hits = raycaster.intersectObject(movedMesh, false).map((h) => ({
        distance: h.distance, point: v3(h.point), faceIndex: h.faceIndex,
        face: { a: h.face!.a, b: h.face!.b, c: h.face!.c, normal: v3(h.face!.normal) },
    }));
    return { origin, dir, side: ['front', 'back', 'double'][side], near, far: far === Infinity ? null : far, firstHitOnly, hits };
});
(raycaster as any).firstHitOnly = false;
const collision = (r: { hit: boolean; point?: THREE.Vector3 | { x: number; y: number; z: number }; normal?: { x: number; y: number; z: number }; distance?: number }) => ({
    hit: r.hit, point: r.point ? v3(r.point) : null, normal: r.normal ? v3(r.normal) : null, distance: r.distance ?? null,
});
out.collision = Array.from({ length: N * 2 }, () => {
    const a = rand() < 0.5 ? surfacePoint().pos : vin(3);
    const b = rand() < 0.3 ? { x: a.x + range(-0.05, 0.05), y: a.y, z: a.z } : vin(3);
    const r = range(0.05, 1.5);
    const kind = pick(['shaft', 'shortBridge', 'bezier']);
    if (kind === 'bezier') {
        const c1 = vin(3), c2 = vin(3);
        const res = rand() < 0.5 ? 8 : Math.floor(range(2, 16));
        return { kind, a, b, c1, c2, r, res, result: collision(checkBezierCollision(a, c1, c2, b, r, knotMesh, undefined, res)) };
    }
    const result = kind === 'shaft' ? checkShaftCollision(a, b, r, knotMesh) : checkShortBridgeCollision(a, b, r, knotMesh);
    return { kind, a, b, r, result: collision(result) };
});

// --- settings ---
/** Sets a field, or deletes it: JSON cannot carry an explicit `undefined`
 *  (which a spread would copy over the default), only an absent key. */
function setOrDelete(o: any, key: string, value: unknown) {
    if (value === undefined) delete o[key]; else o[key] = value;
}
function randomSettings() {
    const s: any = createDefaultSettings();
    setOrDelete(s.tip, 'coneAngleMode', pick([...modes, undefined]));
    setOrDelete(s.tip, 'adaptiveConeAngleOffsetDeg', rand() < 0.1 ? undefined : range(-20, 120));
    s.tip.contactDiameterMm = range(-0.1, 6);
    s.tip.bodyDiameterMm = range(0.3, 3);
    s.tip.lengthMm = range(0, 60);
    s.tip.penetrationMm = range(0, 0.3);
    setOrDelete(s.tip, 'diskThicknessMm', rand() < 0.2 ? undefined : range(0.02, 0.4));
    s.tip.maxStandoffMm = rand() < 0.2 ? 1.5 : range(0.1, 2);
    s.shaft.diameterMm = range(0, 25);
    setOrDelete(s.shaft, 'secondaryDiameterMm', rand() < 0.3 ? undefined : range(0.3, 3));
    setOrDelete(s.shaft, 'maxAngleDeg', rand() < 0.2 ? undefined : range(10, 89));
    s.shaft.isStraight = rand() < 0.5;
    s.shaft.routingAlgorithm = pick(['astar', 'potential']);
    s.roots.diameterMm = range(-1, 60);
    s.roots.diskHeightMm = range(-1, 25);
    s.roots.coneHeightMm = range(-1, 60);
    s.roots.neckBlend = range(0, 1);
    s.baseFlare.enabled = rand() < 0.5;
    s.grid.enabled = rand() < 0.5;
    s.grid.spacingMm = range(1, 8);
    s.autoSupport.areaPerSupportMm2 = range(-5, 40);
    s.autoSupport.sizeScale = range(0, 3);
    s.autoSupport.coverageTargetPercent = range(60, 110);
    s.autoSupport.leafFanMaxAngleDeg = range(10, 90);
    s.autoSupport.suctionAreaExponent = range(-0.1, 0.5);
    s.autoSupport.sizingPreset = pick(['detail', 'structure', 'anchor', 'mine', '  ']);
    s.autoBracing.braceDiameterMm = range(0, 3);
    s.autoBracing.seedJitterMm = range(-2, 25);
    s.autoBracing.patternIntervalMm = range(0.5, 60);
    return s;
}
out.settings = Array.from({ length: N }, () => {
    const raw = randomSettings();
    const rawCopy = structuredClone(raw);
    setSettings(raw);
    const normalized = structuredClone(getSettings());
    const hex = encodeSupportSettingsHex(normalized);
    const rawHex = encodeSupportSettingsHex(rawCopy);
    return { raw: rawCopy, normalized, hex, rawHex, decoded: decodeSupportSettingsHex(hex, normalized) };
});

// --- grid math ---
out.grid = Array.from({ length: N }, () => {
    const x = range(-50, 50), y = range(-50, 50), s = pick([0, -1, range(0.5, 6)]);
    const key = gridNodeKeyFromXY(x, y, s);
    return { x, y, s, idx: snapToGridIndex(x, s), key, xy: gridSnappedXYFromKey(key, s) };
});

// --- placements and builders ---
function settingsForBuild() {
    const raw = randomSettings();
    setSettings(raw);
    return structuredClone(getSettings());
}
function tipInput() {
    if (rand() < 0.6) return surfacePoint();
    return { pos: vin(2), normal: unit() };
}
const trunks: unknown[] = [];
const placements: unknown[] = [];
for (let i = 0; i < N; i++) {
    const settings = settingsForBuild();
    const tip = tipInput();
    const overrides = rand() < 0.5 ? undefined : {
        rootsDiameterMm: rand() < 0.5 ? undefined : range(1, 4),
        rootsDiskHeightMm: rand() < 0.5 ? undefined : range(0, 1),
        rootsConeHeightMm: rand() < 0.5 ? undefined : range(0, 2),
        shaftDiameterMm: rand() < 0.5 ? undefined : range(0.5, 2),
        tipContactDiameterMm: rand() < 0.5 ? undefined : range(0.1, 0.6),
        tipBodyDiameterMm: rand() < 0.5 ? undefined : range(0.5, 2),
        tipLengthMm: rand() < 0.5 ? undefined : range(1, 4),
        tipDiskLengthOverrideMm: rand() < 0.7 ? undefined : range(0, 0.5),
    };
    const isSmallIsland = rand() < 0.2;
    const tipProfile = {
        type: 'disk' as const, contactDiameterMm: settings.tip.contactDiameterMm, bodyDiameterMm: settings.tip.bodyDiameterMm,
        lengthMm: settings.tip.lengthMm, penetrationMm: settings.tip.penetrationMm,
        diskThicknessMm: settings.tip.diskThicknessMm ?? 0.1, maxStandoffMm: settings.tip.maxStandoffMm ?? 1.5,
        standoffAngleThreshold: settings.tip.standoffAngleThreshold ?? Math.PI / 4,
    };
    const placementInput = { tipPos: tip.pos, tipNormal: tip.normal, tipProfile, rootsTopZ: range(0, 3), isSmallIsland };
    placements.push({ settings, input: placementInput, result: calculateStandardPlacement(placementInput) });

    const input = { tipPos: tip.pos, tipNormal: tip.normal, modelId: 'm', overrides, isSmallIsland };
    if (rand() < 0.5) {
        __resetUuid();
        trunks.push({ op: 'buildTrunkData', settings, input, result: buildTrunkData(input) });
    } else {
        // An authored placement, as a router would hand back.
        const socket = { x: tip.pos.x + range(-3, 3), y: tip.pos.y + range(-3, 3), z: tip.pos.z - range(0, 4) };
        const nJ = Math.floor(range(0, 4));
        const joints = Array.from({ length: nJ }, () => ({ x: socket.x + range(-4, 4), y: socket.y + range(-4, 4), z: range(-1, socket.z + 1) }));
        const nC = Math.floor(range(0, 3));
        const constructionJoints = Array.from({ length: nC }, () => ({ x: socket.x, y: socket.y, z: range(-1, socket.z + 1) }));
        const basePos = { x: socket.x + range(-2, 2), y: socket.y + range(-2, 2), z: 0 };
        const placement = {
            basePos, socketPos: rand() < 0.3 ? undefined : socket,
            unsnappedBottomPos: rand() < 0.5 ? undefined : { x: basePos.x + (rand() < 0.5 ? 0 : 0.5), y: basePos.y, z: 0 },
            snappedNodeKey: pick([undefined, null, '', `${Math.floor(range(-5, 5))},${Math.floor(range(-5, 5))}`]),
            gridIgnored: rand() < 0.2 ? true : undefined,
            joints: rand() < 0.2 ? undefined : joints,
            constructionJoints: rand() < 0.2 ? undefined : constructionJoints,
            error: rand() < 0.15 ? 'COLLISION_WITH_MODEL' : undefined,
            warning: rand() < 0.2 ? 'ANGLE_VERTICAL_WARNING' : undefined,
            angle: rand() < 0.2 ? undefined : range(80, 180),
            coneAxis: rand() < 0.3 ? undefined : unit(),
            stagnated: rand() < 0.1 ? true : undefined,
            exhaustedBudget: rand() < 0.1 ? true : undefined,
        };
        if (!placement.socketPos) placement.socketPos = calculateStandardPlacement(placementInput).socketPos;
        const withMesh = rand() < 0.5;
        __resetUuid();
        trunks.push({
            op: 'buildTrunkDataFromPlacement', settings, input, placement, withMesh,
            result: buildTrunkDataFromPlacement({ ...input, mesh: withMesh ? knotMesh : undefined }, placement as never),
        });
    }
}
out.placements = placements;
out.trunks = trunks;

out.branches = Array.from({ length: N }, () => {
    const live = settingsForBuild();
    const tip = tipInput();
    const parentKnot = { id: 'k1', parentShaftId: 's1', pos: { x: tip.pos.x + range(-6, 6), y: tip.pos.y + range(-6, 6), z: tip.pos.z - range(0, 10) } };
    const own = rand() < 0.3 ? settingsForBuild() : undefined;
    setSettings(live);
    const tipProfile = rand() < 0.2 ? {
        type: 'disk' as const, contactDiameterMm: range(0.1, 0.6), bodyDiameterMm: range(0.5, 2), lengthMm: range(1, 4),
        penetrationMm: 0.05, diskThicknessMm: 0.1, maxStandoffMm: rand() < 0.5 ? 1.5 : range(0.1, 1), standoffAngleThreshold: Math.PI / 4,
    } : undefined;
    const input = {
        tipPos: tip.pos, tipNormal: tip.normal, modelId: 'm', parentKnot,
        shaftDiameterMm: rand() < 0.5 ? undefined : range(0.5, 2),
        tipContactDiameterMm: rand() < 0.5 ? undefined : range(0.1, 0.6),
        rootsDiameterMm: rand() < 0.5 ? undefined : range(1, 4),
        settings: own, tipProfile,
    };
    const withMesh = rand() < 0.6;
    __resetUuid();
    const result = buildBranchData({ ...input, mesh: withMesh ? knotMesh : undefined });
    // A rebuilt copy re-stamped with the first build's ids.
    __resetUuid();
    const rebuilt = buildBranchData({ ...input, tipPos: { x: tip.pos.x + 0.3, y: tip.pos.y, z: tip.pos.z }, mesh: withMesh ? knotMesh : undefined });
    return { live, input, withMesh, result, rebuilt, remapped: remapBranchGeometryIds(rebuilt.branch, result.branch) };
});

out.leaves = Array.from({ length: N }, () => {
    const settings = settingsForBuild();
    const tip = tipInput();
    const parentKnot = { id: 'k1', parentShaftId: 's1', pos: { x: tip.pos.x + range(-3, 3), y: tip.pos.y + range(-3, 3), z: tip.pos.z - range(0, 6) } };
    const input = {
        tipPos: tip.pos, surfaceNormal: tip.normal, modelId: 'm', parentKnot, hostDiameterMm: range(0.5, 2),
        tipContactDiameterMm: rand() < 0.5 ? undefined : range(0.1, 0.6),
    };
    const withMesh = rand() < 0.6;
    __resetUuid();
    return { settings, input, withMesh, result: buildLeafData({ ...input, mesh: withMesh ? knotMesh : undefined }) };
});

process.stdout.write(JSON.stringify(out));
realLog('');
