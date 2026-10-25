// Parity oracle for block 2 of the native port: three-mesh-bvh's tree and
// closest point, DragonFruit's SDFCache/ColumnClearanceMap, the shaft-angle and
// joint constraint solvers, curveUtils and moveJoint. Every operation is
// recorded in call order, because the SDF cache makes answers depend on what
// was asked before. Output: libs/sc-geom/support-gen-df/tests/fixtures/df_sdf.json
//
// Run with run_df_script.sh from a DragonFruit sandbox root.
import * as THREE from 'three';
import { readFileSync } from 'node:fs';
import { STLLoader } from 'three/examples/jsm/loaders/STLLoader.js';
import { initializeBVH, accelerateGeometry } from './src/utils/bvh';
import { SDFCache } from './src/supports/PlacementLogic/Pathfinding/SDFCache';
import { ColumnClearanceMap } from './src/supports/PlacementLogic/Pathfinding/ColumnClearanceMap';
import { clampShaftAngle } from './src/supports/PlacementLogic/ShaftAngleConstraint';
import { solveJointConstraint } from './src/supports/PlacementLogic/JointConstraintSolver';
import {
    updateCurvesAtJoint, updateSegmentTension, updateSegmentBias, removeSegmentCurve,
    updateCurveTension, removeCurveAtJoint, ensureBezierSegment,
} from './src/supports/Curves/curveUtils';
import { moveJoint } from './src/supports/SupportPrimitives/Joint/jointUtils';
import { isCollisionFrustumBlocked, calculateSafeOffset, isContactConeBlocked } from './src/supports/PlacementLogic/CollisionAvoidance';
import { getOrCreateSDFCache } from './src/supports/PlacementLogic/Pathfinding/SDFCachePool';
import { setSettings } from './src/supports/Settings/state';
import { createDefaultSettings } from './src/supports/Settings/types';

const realLog = console.log;
console.log = () => {};
initializeBVH();

let seed = 0xb10c2;
function rand(): number {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const range = (lo: number, hi: number) => lo + (hi - lo) * rand();
const vin = (box: THREE.Box3, pad: number) => ({
    x: range(box.min.x - pad, box.max.x + pad),
    y: range(box.min.y - pad, box.max.y + pad),
    z: range(box.min.z - pad, box.max.z + pad),
});
const unit = () => {
    const p = new THREE.Vector3(range(-1, 1), range(-1, 1), range(-1, 1));
    if (p.lengthSq() < 1e-6) p.set(0, 0, 1);
    p.normalize();
    return { x: p.x, y: p.y, z: p.z };
};
const v3 = (v: THREE.Vector3) => ({ x: v.x, y: v.y, z: v.z });

/** A position-only, non-indexed geometry, as STLLoader produces. */
function soup(source: THREE.BufferGeometry): THREE.BufferGeometry {
    const flat = source.index ? source.toNonIndexed() : source;
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.Float32BufferAttribute(Array.from(flat.attributes.position.array), 3));
    return g;
}

function meshes(): { name: string; geometry: THREE.BufferGeometry; matrix: THREE.Matrix4 }[] {
    const cubeBuf = readFileSync('../corpus-cube/cubo-rx30-ry30-z10.stl');
    const cube = new STLLoader().parse(cubeBuf.buffer.slice(cubeBuf.byteOffset, cubeBuf.byteOffset + cubeBuf.byteLength) as ArrayBuffer);
    const knot = soup(new THREE.TorusKnotGeometry(8, 2.5, 96, 10));
    knot.translate(0, 0, 14);
    // An open, folded sheet: no inside, and creases where two faces tie.
    const sheet = soup(new THREE.PlaneGeometry(20, 20, 6, 6));
    const pos = sheet.attributes.position;
    for (let i = 0; i < pos.count; i++) pos.setZ(i, Math.abs(pos.getX(i)) * 0.6 + Math.round(pos.getY(i)) * 0.0);
    const moved = new THREE.Matrix4().compose(
        new THREE.Vector3(3, -2, 5), new THREE.Quaternion().setFromEuler(new THREE.Euler(0.3, -0.2, 0.7)), new THREE.Vector3(1.25, 1.25, 1.25),
    );
    // Bounding-box centres on integers, so every split plane meets centres
    // exactly: the partition's tie rule (`>= pos` goes right) decides the tree.
    const tieCoords: number[] = [];
    for (let i = 0; i < 41; i++) {
        // Many centres exactly on each split plane, met from both ends.
        const k = [0, 20, 40, 20][i % 4] + ((i * 17) % 41 < 8 ? 10 : 0);
        const y = (k * 7) % 5;
        const z = (k * 3) % 4;
        tieCoords.push(k - 0.5, y, z, k + 0.5, y + 1, z, k, y, z + 2);
    }
    const ties = new THREE.BufferGeometry();
    ties.setAttribute('position', new THREE.Float32BufferAttribute(tieCoords, 3));
    return [
        { name: 'ties', geometry: ties, matrix: new THREE.Matrix4() },
        { name: 'cube', geometry: soup(cube), matrix: new THREE.Matrix4() },
        { name: 'torusKnot', geometry: knot, matrix: new THREE.Matrix4() },
        { name: 'torusKnotMoved', geometry: soup(new THREE.TorusKnotGeometry(8, 2.5, 96, 10)), matrix: moved },
        { name: 'sheet', geometry: sheet, matrix: new THREE.Matrix4() },
    ];
}

const out: Record<string, unknown> = { dragonfruit: 'f331fc555abb31c4bcb28d1a358edb887f0afe07', meshes: [] };

for (const { name, geometry, matrix } of meshes()) {
    const positions = Array.from(geometry.attributes.position.array as Float32Array);
    accelerateGeometry(geometry);
    const mesh = new THREE.Mesh(geometry);
    mesh.matrixAutoUpdate = false;
    mesh.matrix.copy(matrix);
    mesh.matrixWorld.copy(matrix);
    const bvh = (geometry as any).boundsTree;
    const worldBox = new THREE.Box3().setFromBufferAttribute(geometry.attributes.position as THREE.BufferAttribute).applyMatrix4(matrix);

    const closest: unknown[] = [];
    const localBox = new THREE.Box3().setFromBufferAttribute(geometry.attributes.position as THREE.BufferAttribute);
    for (let i = 0; i < 120; i++) {
        const p = vin(localBox, 4);
        const maxT = rand() < 0.5 ? Infinity : range(0.2, 4);
        const minT = rand() < 0.8 ? 0 : range(0, 1);
        const target = { point: new THREE.Vector3(), distance: 0, faceIndex: -1 };
        const r = bvh.closestPointToPoint(new THREE.Vector3(p.x, p.y, p.z), target, minT, maxT);
        closest.push({ p, minT, maxT: maxT === Infinity ? null : maxT, result: r ? { point: v3(r.point), distance: r.distance, faceIndex: r.faceIndex } : null });
    }

    const sdf = new SDFCache(mesh, { cellSize: 0.5 });
    const ops: unknown[] = [];
    for (let i = 0; i < 700; i++) {
        const kind = Math.floor(range(0, 8));
        const p = vin(worldBox, 3);
        switch (kind) {
            case 0: ops.push({ op: 'exactSignedDistanceAt', p, result: sdf.exactSignedDistanceAt(p.x, p.y, p.z) }); break;
            case 1: ops.push({ op: 'distanceAt', p, result: sdf.distanceAt(p.x, p.y, p.z) }); break;
            case 2: { const b = range(0.5, 10); ops.push({ op: 'boundedDistanceAt', p, b, result: sdf.boundedDistanceAt(p.x, p.y, p.z, b) }); break; }
            case 3: ops.push({ op: 'distanceAtTrilinear', p, result: sdf.distanceAtTrilinear(p.x, p.y, p.z) }); break;
            case 4: {
                const m = rand() < 0.5 ? Infinity : range(0.2, 3);
                const r = sdf.distanceAndGradientAt(p.x, p.y, p.z, m);
                ops.push({ op: 'distanceAndGradientAt', p, m: m === Infinity ? null : m, result: { distance: r.distance === Infinity ? null : r.distance, gradient: r.gradient } });
                break;
            }
            case 5: { const m = range(0.2, 3); const r = sdf.distanceAtWithin(p.x, p.y, p.z, m); ops.push({ op: 'distanceAtWithin', p, m, result: r === Infinity ? null : r }); break; }
            default: {
                const q = rand() < 0.3 ? { x: p.x, y: p.y, z: p.z - range(0, 30) } : vin(worldBox, 3);
                const c = range(0.1, 1.5);
                if (kind === 7 && rand() < 0.3) {
                    const enabled = sdf.enableColumnMap(c, 0.2);
                    ops.push({ op: 'enableColumnMap', c, cell: 0.2, result: enabled });
                }
                ops.push({ op: 'segmentBlocked', a: p, b: q, c, result: sdf.segmentBlocked(p.x, p.y, p.z, q.x, q.y, q.z, c) });
            }
        }
    }

    const column = ColumnClearanceMap.build(mesh, 0.6, 0.25);
    const columnSamples: unknown[] = [];
    if (column) {
        for (let i = 0; i < 200; i++) {
            const p = vin(worldBox, 2);
            const zb = p.z - range(0, 30);
            const lo = column.lowestBlockedZ(p.x, p.y);
            columnSamples.push({ p, zb, lowest: lo, verdict: column.columnVerdict(p.x, p.y, p.z, zb) });
        }
    }

    (out.meshes as unknown[]).push({
        name, positions, matrix: Array.from(matrix.elements),
        index: Array.from(geometry.index!.array as ArrayLike<number>),
        closest, ops, stats: sdf.stats, size: sdf.size,
        column: column ? { width: column.stats.width, height: column.stats.height, radiusMm: column.stats.radiusMm, samples: columnSamples } : null,
    });
}

// --- solvers, curves and moveJoint, against the torus knot's pooled field ---
const settings = createDefaultSettings();
setSettings(settings);
const knotGeom = soup(new THREE.TorusKnotGeometry(8, 2.5, 96, 10));
knotGeom.translate(0, 0, 14);
const knotPositions = Array.from(knotGeom.attributes.position.array as Float32Array);
accelerateGeometry(knotGeom);
const knotMesh = new THREE.Mesh(knotGeom);
knotMesh.updateMatrixWorld(true);
const knotBox = new THREE.Box3().setFromBufferAttribute(knotGeom.attributes.position as THREE.BufferAttribute);

const solvers: Record<string, unknown[]> = {
    clampShaftAngle: [], solveJointConstraint: [], curves: [], moveJoint: [], collision: [],
};

function trunkFixture(i: number) {
    const base = vin(knotBox, 3);
    base.z = range(0, 2);
    const segs = 1 + Math.floor(range(0, 4));
    const segments = [] as Record<string, unknown>[];
    let z = base.z + 1.5;
    for (let k = 0; k < segs; k++) {
        z += range(1, 6);
        const seg: Record<string, unknown> = { id: `s${k}`, diameter: range(0.8, 2) };
        if (k < segs - 1 || rand() < 0.5) seg.topJoint = { id: `j${k}`, pos: { x: base.x + range(-2, 2), y: base.y + range(-2, 2), z }, diameter: 1.2 };
        if (k > 0 && rand() < 0.7) seg.bottomJoint = { ...(segments[k - 1].topJoint as object) } as never;
        if (rand() < 0.35) Object.assign(seg, {
            type: 'bezier', controlPoint1: vin(knotBox, 3), controlPoint2: vin(knotBox, 3),
            startTangent: rand() < 0.2 ? { x: 0, y: 0, z: 0 } : unit(), endTangent: rand() < 0.2 ? { x: 0, y: 0, z: 0 } : unit(),
            tension: range(0, 2), bias: range(0, 1), resolution: 16,
        });
        segments.push(seg);
    }
    const cone = rand() < 0.85 ? {
        id: `c${i}`, pos: { x: base.x + range(-1, 1), y: base.y + range(-1, 1), z: z + range(1, 4) },
        normal: unit(), surfaceNormal: rand() < 0.2 ? undefined : unit(),
        profile: {
            type: rand() < 0.85 ? 'disk' : undefined, contactDiameterMm: range(0.2, 0.8), bodyDiameterMm: range(0.8, 2),
            lengthMm: range(1, 4), penetrationMm: 0.1, diskThicknessMm: 0.1,
            maxStandoffMm: rand() < 0.3 ? 1.5 : rand() < 0.05 ? undefined : range(0.2, 1.2),
            standoffAngleThreshold: Math.PI / 4,
        },
        socketJointId: segments.length > 0 && segments[segments.length - 1].topJoint ? (segments[segments.length - 1].topJoint as { id: string }).id : undefined,
    } : undefined;
    const root = {
        id: 'r', modelId: 'm', transform: { pos: base, rot: { x: 0, y: 0, z: 0, w: 1 } },
        diameter: 3, diskHeight: rand() < 0.1 ? 0 : range(0.2, 1), coneHeight: rand() < 0.1 ? 0 : range(0.5, 2),
    };
    return { trunk: { id: `t${i}`, modelId: 'm', typeId: 'trunk', rootId: 'r', segments, contactCone: cone }, root };
}

for (let i = 0; i < 300; i++) {
    const start = vin(knotBox, 5);
    const end = vin(knotBox, 5);
    const axis = rand() < 0.1 ? { x: 0, y: 0, z: 0 } : rand() < 0.5 ? { x: 0, y: 0, z: rand() < 0.5 ? 1 : -1 } : unit();
    const maxA = range(10, 89);
    solvers.clampShaftAngle.push({ start, end, maxA, axis, result: clampShaftAngle(start, end, maxA, axis) });

    const { trunk, root } = trunkFixture(i);
    const joints = trunk.segments.flatMap((s) => [s.topJoint, s.bottomJoint]).filter(Boolean) as { id: string }[];
    const jointId = joints.length && rand() < 0.95 ? joints[Math.floor(range(0, joints.length))].id : 'missing';
    const target = vin(knotBox, 4);
    const parentStart = rand() < 0.3 ? vin(knotBox, 3) : undefined;
    const giveRoot = rand() < 0.8;
    solvers.solveJointConstraint.push({
        trunk, root, jointId, target, maxA, parentStart, giveRoot,
        result: solveJointConstraint(trunk as never, jointId, target, maxA, giveRoot ? root as never : undefined, parentStart),
    });

    const op = Math.floor(range(0, 7));
    const segId = trunk.segments.length ? (trunk.segments[Math.floor(range(0, trunk.segments.length))] as { id: string }).id : 'missing';
    const tension = range(0, 2.5);
    const bias = range(0, 1);
    const force = rand() < 0.5;
    const curveResult = (() => {
        switch (op) {
            case 0: return updateCurvesAtJoint(structuredClone(trunk) as never, jointId, root as never, force);
            case 1: return updateSegmentTension(structuredClone(trunk) as never, segId, tension, root as never);
            case 2: return updateSegmentBias(structuredClone(trunk) as never, segId, bias, root as never);
            case 3: return removeSegmentCurve(structuredClone(trunk) as never, segId);
            case 4: return updateCurveTension(structuredClone(trunk) as never, jointId, tension, root as never);
            case 5: return removeCurveAtJoint(structuredClone(trunk) as never, jointId);
            default: return { ...structuredClone(trunk), segments: [ensureBezierSegment(structuredClone(trunk.segments[0] ?? { id: 'x', diameter: 1 }) as never)] };
        }
    })();
    solvers.curves.push({ op, trunk, root, jointId, segId, tension, bias, force, result: curveResult });

    const withMesh = rand() < 0.5;
    const curveMode = rand() < 0.3;
    const skipCone = rand() < 0.15;
    const maxAngle = rand() < 0.2 ? undefined : range(30, 85);
    settings.shaft.maxAngleDeg = maxAngle as number;
    setSettings(settings);
    const moved = moveJoint(
        structuredClone(trunk) as never, jointId, target, withMesh ? knotMesh : undefined, curveMode,
        giveRoot ? root as never : undefined, parentStart, { skipContactConeSolve: skipCone },
    );
    solvers.moveJoint.push({ trunk, root, jointId, target, withMesh, curveMode, skipCone, maxAngle, giveRoot, parentStart, result: moved });

    const cs = rand();
    const pa = vin(knotBox, 3);
    const pb = vin(knotBox, 3);
    if (cs < 0.4) {
        const r0 = range(0.05, 1.5);
        const r1 = range(0.05, 1.5);
        const n = rand() < 0.5 ? undefined : Math.floor(range(1, 8));
        solvers.collision.push({ op: 'frustum', a: pa, b: pb, r0, r1, n, result: isCollisionFrustumBlocked(pa, pb, r0, r1, knotMesh, n) });
    } else if (cs < 0.7) {
        const nrm = unit();
        const radius = range(0.2, 1.5);
        const minO = range(0, 0.4);
        const maxO = minO + range(0, 2);
        const frustum = rand() < 0.5 ? undefined : { startRadius: range(0.1, 1), endRadius: range(0.1, 1.5) };
        solvers.collision.push({ op: 'safeOffset', a: pa, n: nrm, b: pb, radius, minO, maxO, frustum, result: calculateSafeOffset(pa, nrm, pb, radius, knotMesh, minO, maxO, 0.2, frustum) });
    } else {
        const sdf = getOrCreateSDFCache(knotMesh);
        const cone = { start: pa, end: pb, startRadius: range(0.05, 0.5), endRadius: range(0.3, 1.5) };
        solvers.collision.push({ op: 'contactCone', cone, result: isContactConeBlocked(sdf, cone) });
    }
}

process.stdout.write(JSON.stringify({ ...out, knotPositions, solvers }));
realLog('');
