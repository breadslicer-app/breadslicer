// Evaluates DragonFruit's own support primitives over deterministic inputs and
// prints the results as JSON: the parity oracle for the native port in
// libs/sc-geom/support-gen-df (tests/df_parity_primitives.rs).
//
// Run from a DragonFruit sandbox root (see run_df_script.sh); imports are
// relative to it. Nothing here is linked into SoulCrafted.
import * as THREE from 'three';
import { calculateDiskThickness, getDiskRotation } from './src/supports/SupportPrimitives/ContactDisk/contactDiskUtils';
import {
    getFinalSocketPosition,
    getContactConeLowestZ,
    getConeQuaternion,
} from './src/supports/SupportPrimitives/ContactCone/contactConeUtils';
import {
    recomputeContactConeForMovedDisk,
    moveDiskKeepingConeConnection,
} from './src/supports/SupportPrimitives/ContactDisk/ContactDiskInteraction';
import {
    calculateBezierControlPoints,
    getBezierPointAtT,
    getBezierTangentAtT,
    subdivideCubicBezier,
    bezierToLineSegments,
    calculateAdaptiveBezierResolution,
} from './src/supports/Curves/BezierUtils';
import { projectOntoSegment, remapKnotAcrossSplit } from './src/supports/SupportPrimitives/Knot/knotUtils';
import { resolveSegmentEndpoints } from './src/supports/SupportPrimitives/Knot/segmentEndpoints';
import { findClosestSegment } from './src/supports/SupportPrimitives/Joint/jointUtils';
import { taperWidthAlong } from './src/supports/SupportPrimitives/Shaft/shaftTaperGeometry';
import { influenceRadiusMm } from './src/supports/autoSupport/constants';
import { selectTypeForPlacement } from './src/supports/supportTypeRegistry';

// mulberry32: a small seeded PRNG, so the inputs are reproducible.
let seed = 0x5c0ffee;
function rand(): number {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const range = (lo: number, hi: number) => lo + (hi - lo) * rand();
const v = (s = 10) => ({ x: range(-s, s), y: range(-s, s), z: range(-s, s) });
const unit = () => {
    const p = new THREE.Vector3(range(-1, 1), range(-1, 1), range(-1, 1));
    if (p.lengthSq() < 1e-6) p.set(0, 0, 1);
    p.normalize();
    return { x: p.x, y: p.y, z: p.z };
};
const q = (x: THREE.Quaternion) => ({ x: x.x, y: x.y, z: x.z, w: x.w });

function diskProfile() {
    const pick = rand();
    return {
        type: 'disk' as const,
        diskThicknessMm: pick < 0.1 ? undefined : range(0.02, 0.4),
        maxStandoffMm: pick < 0.2 ? 1.5 : pick < 0.25 ? undefined : range(0.05, 1.2),
        standoffAngleThreshold: pick > 0.9 ? undefined : range(0.1, 1.3),
        contactDiameterMm: pick > 0.8 ? undefined : range(0.05, 2),
    };
}

function tipProfile() {
    const disk = diskProfile();
    const kind = rand();
    return {
        type: kind < 0.8 ? ('disk' as const) : kind < 0.9 ? ('sphere' as const) : undefined,
        contactDiameterMm: range(0.1, 1.5),
        bodyDiameterMm: range(0.5, 2.5),
        lengthMm: range(0.5, 5),
        penetrationMm: range(0, 0.3),
        diskThicknessMm: disk.diskThicknessMm,
        maxStandoffMm: disk.maxStandoffMm,
        standoffAngleThreshold: disk.standoffAngleThreshold,
        sphereRadiusRatio: kind >= 0.8 && kind < 0.9 ? range(1, 2) : undefined,
    };
}

function cone(i: number) {
    return {
        id: `c${i}`,
        pos: v(),
        normal: unit(),
        surfaceNormal: rand() < 0.2 ? undefined : unit(),
        diskLengthOverride: rand() < 0.2 ? range(0, 0.5) : undefined,
        profile: tipProfile(),
    };
}

const N = 150;
const out: Record<string, unknown[]> = {
    calculateDiskThickness: [],
    getFinalSocketPosition: [],
    getContactConeLowestZ: [],
    getConeQuaternion: [],
    getDiskRotation: [],
    recomputeContactConeForMovedDisk: [],
    moveDiskKeepingConeConnection: [],
    calculateBezierControlPoints: [],
    getBezierPointAtT: [],
    getBezierTangentAtT: [],
    subdivideCubicBezier: [],
    bezierToLineSegments: [],
    calculateAdaptiveBezierResolution: [],
    projectOntoSegment: [],
    remapKnotAcrossSplit: [],
    resolveSegmentEndpoints: [],
    findClosestSegment: [],
    taperWidthAlong: [],
    influenceRadiusMm: [],
    selectTypeForPlacement: [],
};

for (let i = 0; i < N; i++) {
    const surfaceNormal = rand() < 0.05 ? { x: 0, y: 0, z: 0 } : unit();
    const coneAxis = unit();
    const profile = diskProfile();
    out.calculateDiskThickness.push({
        surfaceNormal, coneAxis, profile,
        result: calculateDiskThickness(surfaceNormal, coneAxis, profile),
    });

    const c = cone(i);
    out.getFinalSocketPosition.push({ cone: c, result: getFinalSocketPosition(c as never) });
    out.getContactConeLowestZ.push({ cone: c, result: getContactConeLowestZ(c as never) });

    const n = rand() < 0.05 ? { x: 0, y: -1, z: 0 } : unit();
    out.getConeQuaternion.push({ normal: n, result: q(getConeQuaternion(n)) });
    out.getDiskRotation.push({ normal: n, result: q(getDiskRotation(n)) });

    const nextPos = v();
    const nextNormal = rand() < 0.05 ? { x: 0, y: 0, z: 0 } : unit();
    const fixedSocket = rand() < 0.5 ? v() : undefined;
    out.recomputeContactConeForMovedDisk.push({
        cone: c, nextContactPos: nextPos, nextSurfaceNormal: nextNormal, fixedSocketPos: fixedSocket,
        result: recomputeContactConeForMovedDisk(c as never, nextPos, nextNormal, fixedSocket),
    });

    const disk = {
        id: `d${i}`, pos: v(), surfaceNormal: unit(),
        coneAxis: rand() < 0.1 ? { x: 0, y: 0, z: 0 } : unit(),
        diskLengthOverride: rand() < 0.5 ? range(0, 1) : undefined,
        profile: diskProfile(), contactDiameterMm: range(0.1, 1),
    };
    out.moveDiskKeepingConeConnection.push({
        disk, nextContactPos: nextPos, nextSurfaceNormal: nextNormal,
        result: moveDiskKeepingConeConnection(disk as never, nextPos, nextNormal),
    });

    const [p0, p1, p2, p3] = [v(), v(), v(), v()];
    const tension = range(-0.5, 2.5);
    const bias = range(0, 1);
    const t = rand();
    out.calculateBezierControlPoints.push({
        p0, p3, startTangent: unit(), endTangent: unit(), tension, bias, result: null,
    });
    const cp = out.calculateBezierControlPoints[i] as Record<string, never>;
    (cp as Record<string, unknown>).result = calculateBezierControlPoints(cp.p0, cp.p3, cp.startTangent, cp.endTangent, tension, bias);
    out.getBezierPointAtT.push({ p0, p1, p2, p3, t, result: getBezierPointAtT(p0, p1, p2, p3, t) });
    out.getBezierTangentAtT.push({ p0, p1, p2, p3, t, result: getBezierTangentAtT(p0, p1, p2, p3, t) });
    out.subdivideCubicBezier.push({ p0, p1, p2, p3, t, result: subdivideCubicBezier(p0, p1, p2, p3, t) });
    const resolution = rand() < 0.5 ? undefined : Math.floor(range(0, 30));
    out.bezierToLineSegments.push({ p0, p1, p2, p3, resolution, result: bezierToLineSegments(p0, p1, p2, p3, resolution) });
    const options = rand() < 0.5 ? undefined : {
        minResolution: range(0, 10), maxResolution: range(0, 40),
        targetChordLengthMm: range(0, 5), curvatureWeight: range(0, 4),
    };
    out.calculateAdaptiveBezierResolution.push({
        p0, p1, p2, p3, options, result: calculateAdaptiveBezierResolution(p0, p1, p2, p3, options),
    });

    const origin = v();
    const direction = unit();
    const segStart = v();
    const segEnd = rand() < 0.05 ? segStart : v();
    const ray = new THREE.Ray(new THREE.Vector3(origin.x, origin.y, origin.z), new THREE.Vector3(direction.x, direction.y, direction.z));
    out.projectOntoSegment.push({
        origin, direction, start: segStart, end: segEnd,
        result: projectOntoSegment(ray, new THREE.Vector3(segStart.x, segStart.y, segStart.z), new THREE.Vector3(segEnd.x, segEnd.y, segEnd.z)),
    });

    const knot = { id: 'k', parentShaftId: rand() < 0.1 ? 'other' : 'seg', t: rand() < 0.1 ? undefined : range(-0.2, 1.2), pos: v() };
    const splitT = rand() < 0.1 ? (rand() < 0.5 ? 0 : 1) : rand();
    out.remapKnotAcrossSplit.push({ knot, splitT, result: remapKnotAcrossSplit(knot, 'seg', 'seg', 'top', splitT) });

    out.taperWidthAlong.push((() => {
        const pointCount = Math.floor(range(0, 8));
        const start = range(0.2, 3);
        const end = rand() < 0.3 ? undefined : (rand() < 0.2 ? start : range(0.2, 3));
        const widthAt = taperWidthAlong(pointCount, start, end);
        return { pointCount, start, end, result: Array.from({ length: pointCount + 2 }, (_, k) => widthAt(k)) };
    })());

    const diffZ = range(-5, 60);
    out.influenceRadiusMm.push({ diffZ, result: influenceRadiusMm(diffZ) });

    const metric = rand() < 0.5 ? 'tipHeight' : 'contactSpan';
    const value = range(-1, 12);
    const cutoff = rand() < 0.5 ? undefined : range(1, 9);
    out.selectTypeForPlacement.push({
        metric, value, cutoff, result: selectTypeForPlacement(metric as never, value, () => cutoff),
    });
}

// Shafted entities: a trunk on a root, a branch on a knot, a stick between two cones.
for (let i = 0; i < N; i++) {
    const segCount = 1 + Math.floor(range(0, 4));
    const segments = Array.from({ length: segCount }, (_, k) => {
        const seg: Record<string, unknown> = { id: `s${k}`, diameter: range(0.5, 2) };
        if (rand() < 0.8 || k < segCount - 1) seg.topJoint = { id: `j${k}`, pos: v(), diameter: 1 };
        if (rand() < 0.3) seg.bottomJoint = { id: `b${k}`, pos: v(), diameter: 1 };
        if (rand() < 0.3) Object.assign(seg, {
            type: 'bezier', controlPoint1: v(), controlPoint2: v(),
            startTangent: unit(), endTangent: unit(), tension: 0.5, bias: 0.5, resolution: 16,
        });
        return seg;
    });
    const root = {
        id: 'r', modelId: 'm', transform: { pos: v(), rot: { x: 0, y: 0, z: 0, w: 1 } },
        diameter: 2, diskHeight: range(0, 1), coneHeight: range(0, 2),
    };
    const hostKnot = { id: 'hk', parentShaftId: 'x', pos: v() };
    const kind = ['trunk', 'branch', 'stick', 'kickstand'][Math.floor(range(0, 4))];
    const entity: Record<string, unknown> = { id: `e${i}`, modelId: 'm', typeId: kind, segments };
    if (kind === 'trunk') Object.assign(entity, { rootId: 'r', contactCone: rand() < 0.8 ? cone(i) : undefined });
    if (kind === 'branch') Object.assign(entity, { parentKnotId: 'hk', contactCone: rand() < 0.8 ? cone(i) : undefined });
    if (kind === 'stick') Object.assign(entity, { contactConeA: cone(i), contactConeB: cone(i + 1) });
    if (kind === 'kickstand') Object.assign(entity, {
        rootId: 'r', hostKnotId: 'hk', hostSegmentId: 'x', hostMinT: 0,
        profile: { bodyDiameterMm: 1, terminalStartDiameterMm: 1, terminalEndDiameterMm: 0.5 },
    });
    const giveRoot = rand() < 0.85;
    const giveKnot = rand() < 0.85;
    const hosts = { root: giveRoot ? root : undefined, hostKnot: giveKnot ? hostKnot : undefined };
    out.resolveSegmentEndpoints.push({
        entity, root, hostKnot, giveRoot, giveKnot,
        result: segments.map((seg, k) => resolveSegmentEndpoints(entity as never, seg as never, k, hosts as never)),
    });
    if (kind === 'trunk') {
        const point = v();
        const found = findClosestSegment(entity as never, root as never, point);
        out.findClosestSegment.push({
            entity, root, point,
            result: found ? { segmentId: found.segment.id, t: found.t, pointOnLine: found.pointOnLine } : null,
        });
    }
}

process.stdout.write(JSON.stringify({ dragonfruit: 'f331fc555abb31c4bcb28d1a358edb887f0afe07', cases: out }));
