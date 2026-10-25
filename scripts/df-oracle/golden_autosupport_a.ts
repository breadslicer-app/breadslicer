// Parity oracle for block 6a of the native port: the auto-support passes the
// orchestrator calls — parameter sizing (with V8's Math.pow/log), the load
// budget, the contact tip cap, coverage and gap fill, candidate generation and
// deduplication, minima reinforcement, pose stability, stabilization anchors and
// the density grid. Islands are synthetic but real-shaped (0.25 mm voxel
// footprints, mesh faces under them, perimeter loops) over six meshes, and calls
// that touch the pooled distance field are recorded in call order.
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_autosupport_a.json
//
// Run with run_df_script.sh from a DragonFruit sandbox root.
import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import './src/supports/state';
import { initializeBVH, accelerateGeometry } from './src/utils/bvh';
import { footprintFromPoints } from './src/volumeAnalysis/Islands/voxelFootprint';
import { createDefaultSettings } from './src/supports/Settings/types';
import { setSettings, getSettings } from './src/supports/Settings/state';
import { normalizeAutoSupportSettings } from './src/supports/autoSupport/settings';
import { sizeParameters, modelSizingFactors, presetForArea } from './src/supports/autoSupport/parameterSizing';
import { computeLoadBudget } from './src/supports/autoSupport/loadBudget';
import { localFreeWidthMm, applyContactTipCaps } from './src/supports/autoSupport/contactTipCap';
import {
    coverageRadiusForArea, computeRegionCoverage, findUncoveredClusters, buildGapFillCandidates,
} from './src/supports/autoSupport/coverage';
import { generateCandidates, candidatesFromIsland, deduplicateCandidates } from './src/supports/autoSupport/candidateGeneration';
import { computeMinimaReinforcementPoints } from './src/supports/autoSupport/minimaReinforcement';
import {
    measurePoseStability, posedPositions, isSlenderPart, steepFlatNeedsCoverage, needsToppleCoverage,
} from './src/supports/autoSupport/poseStability';
import { computeStabilizationAnchors } from './src/supports/autoSupport/stabilization';
import {
    generateGridCandidates, computeRegionSpacing, shouldUseDensityGrid, steepFlatSpacingMultiplier,
    steepFlatAnchorBandTop, erodeFootprint, buildBoundaryPoints, sampleBoundary2D, createVoxelSurfaceAt,
    createTriangleSurfaceAt, samplePerimeterLoops,
} from './src/supports/autoSupport/gridPlacement';
import { setSupportBlockedTriangles } from './src/supports/autoSupport/supportBlockers';
import { getOrCreateSDFCache } from './src/supports/PlacementLogic/Pathfinding/SDFCachePool';

const realLog = console.log;
console.log = () => {};
console.warn = () => {};
initializeBVH();

let seed = 0xa070a;
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
type Scene = { mesh: THREE.Mesh; positions: number[]; index: number[] | null; side: 'front' | 'back' | 'double' };
function asMesh(geometry: THREE.BufferGeometry, matrix: THREE.Matrix4 | null, side: 'front' | 'back' | 'double'): Scene {
    // three-mesh-bvh builds one root per geometry group; a mesh loaded from STL
    // or 3MF has none, and neither does the port's BVH.
    geometry.clearGroups();
    const positions = Array.from(geometry.attributes.position.array as Float32Array);
    const index = geometry.index ? Array.from(geometry.index.array as ArrayLike<number>) : null;
    accelerateGeometry(geometry);
    const material = new THREE.MeshBasicMaterial({
        side: side === 'front' ? THREE.FrontSide : side === 'back' ? THREE.BackSide : THREE.DoubleSide,
    });
    const mesh = new THREE.Mesh(geometry, material);
    if (matrix) matrix.decompose(mesh.position, mesh.quaternion, mesh.scale);
    mesh.updateMatrixWorld(true);
    return { mesh, positions, index, side };
}
const posed = (euler: THREE.Euler, position: THREE.Vector3) =>
    new THREE.Matrix4().compose(position, new THREE.Quaternion().setFromEuler(euler), new THREE.Vector3(1, 1, 1));
function tableGeometry() {
    const parts = [new THREE.BoxGeometry(20, 14, 1.5).translate(0, 0, 14)];
    for (const [x, y] of [[-9, -6], [9, -6], [-9, 6], [9, 6]]) parts.push(new THREE.BoxGeometry(1.5, 1.5, 13.5).translate(x, y, 6.75));
    parts.push(new THREE.BoxGeometry(8, 6, 6).translate(-2, 1, 3));
    return mergeGeometries(parts)!;
}
function hangingCone() {
    // A cone hanging point-down above a slab: its tip is a minima.
    const cone = new THREE.ConeGeometry(4, 6, 24).rotateX(Math.PI).translate(0, 0, 9);
    const slab = new THREE.BoxGeometry(14, 14, 2).translate(0, 0, 13);
    return mergeGeometries([cone.toNonIndexed(), slab.toNonIndexed()])!;
}
const scenes: Record<string, Scene> = {
    knot: asMesh(new THREE.TorusKnotGeometry(8, 2.5, 96, 10).translate(0, 0, 14), null, 'front'),
    table: asMesh(tableGeometry(), null, 'double'),
    cubeCorner: asMesh(new THREE.BoxGeometry(20, 20, 20), posed(new THREE.Euler(Math.PI / 4, Math.atan(Math.SQRT1_2), 0), new THREE.Vector3(1, -2, 17.32)), 'front'),
    cubeEdge: asMesh(new THREE.BoxGeometry(20, 20, 20), posed(new THREE.Euler(Math.PI / 4, 0, 0), new THREE.Vector3(0, 0, 14.14)), 'front'),
    plank: asMesh(new THREE.BoxGeometry(2, 30, 40).translate(0, 0, 20), posed(new THREE.Euler(0.12, 0, 0.3), new THREE.Vector3(0, 0, 0.5)), 'front'),
    cone: asMesh(hangingCone(), null, 'back'),
};
const sceneNames = Object.keys(scenes);
out.meshes = Object.fromEntries(Object.entries(scenes).map(([name, s]) => [name, {
    positions: s.positions, index: s.index, matrix: s.mesh.matrixWorld.elements.slice(), side: s.side,
}]));

/** World-space triangle t of a scene, in the BVH-reordered index DragonFruit reads. */
function worldTriangle(scene: Scene, t: number) {
    const g = scene.mesh.geometry;
    const idx = g.index!;
    const pos = g.attributes.position as THREE.BufferAttribute;
    const v = (k: number) => new THREE.Vector3().fromBufferAttribute(pos, idx.getX(t * 3 + k)).applyMatrix4(scene.mesh.matrixWorld);
    return [v(0), v(1), v(2)];
}
function surfacePoint(scene: Scene, wantDown: boolean) {
    const triCount = scene.mesh.geometry.index!.count / 3;
    for (let attempt = 0; ; attempt++) {
        const t = Math.floor(rand() * triCount);
        const [a, b, c] = worldTriangle(scene, t);
        const n = new THREE.Triangle(a, b, c).getNormal(new THREE.Vector3());
        if (wantDown && n.z > -0.2 && attempt < 300) continue;
        let u = rand(), w = rand();
        if (u + w > 1) { u = 1 - u; w = 1 - w; }
        const p = a.clone().addScaledVector(b.clone().sub(a), u).addScaledVector(c.clone().sub(a), w);
        return { pos: v3(p), normal: v3(n), tri: t };
    }
}

// --- islands ---
function footprintAround(cx: number, cy: number, z: number, withZ: boolean) {
    const shape = pick(['rect', 'disc', 'ring', 'ell', 'line', 'speck'] as const);
    const w = shape === 'speck' ? range(0.1, 0.6) : range(0.6, 14);
    const h = shape === 'line' ? range(0.2, 1.4) : shape === 'speck' ? range(0.1, 0.6) : range(0.6, 14);
    const slopeX = range(-0.6, 0.6), slopeY = range(-0.6, 0.6);
    const ox = Math.round(cx * 4) / 4, oy = Math.round(cy * 4) / 4;
    const pts: Array<{ x: number; y: number; z?: number }> = [];
    for (let i = -Math.ceil(w * 2); i <= Math.ceil(w * 2); i++) {
        for (let j = -Math.ceil(h * 2); j <= Math.ceil(h * 2); j++) {
            const dx = i * 0.25, dy = j * 0.25;
            const nx = dx / (w / 2), ny = dy / (h / 2);
            let inside = Math.abs(nx) <= 1 && Math.abs(ny) <= 1;
            if (shape === 'disc') inside = nx * nx + ny * ny <= 1;
            if (shape === 'ring') inside = nx * nx + ny * ny <= 1 && nx * nx + ny * ny >= 0.3;
            if (shape === 'ell') inside = inside && !(nx > 0 && ny > 0);
            if (!inside) continue;
            const p: { x: number; y: number; z?: number } = { x: ox + dx, y: oy + dy };
            if (withZ) p.z = z + slopeX * dx + slopeY * dy;
            pts.push(p);
        }
    }
    if (pts.length === 0) pts.push(withZ ? { x: ox, y: oy, z } : { x: ox, y: oy });
    return { pts, w, h, ox, oy };
}
function trianglesUnder(scene: Scene, ox: number, oy: number, w: number, h: number): number[] {
    const triCount = scene.mesh.geometry.index!.count / 3;
    const ids: number[] = [];
    for (let t = 0; t < triCount; t++) {
        const [a, b, c] = worldTriangle(scene, t);
        const cx = (a.x + b.x + c.x) / 3, cy = (a.y + b.y + c.y) / 3;
        const n = new THREE.Triangle(a, b, c).getNormal(new THREE.Vector3());
        if (n.z < -0.05 && Math.abs(cx - ox) <= w / 2 + 1 && Math.abs(cy - oy) <= h / 2 + 1) ids.push(t);
    }
    return ids;
}
let islandSerial = 0;
function randomIsland(scene: Scene): any {
    const s = surfacePoint(scene, rand() < 0.85);
    const source = pick(['voxel', 'minima', 'overhang', 'overhang'] as const);
    const withZ = source === 'overhang' || rand() < 0.3;
    const fp = footprintAround(s.pos.x, s.pos.y, s.pos.z, withZ);
    const island: any = {
        id: `${source[0]}${islandSerial++}`,
        source,
        contact: new THREE.Vector3(s.pos.x, s.pos.y, s.pos.z),
        baseZ: s.pos.z,
    };
    const cls = pick([undefined, 'intersection', 'voxelOnly', 'minimaOnly'] as const);
    if (cls) island.class = cls;
    if (source !== 'minima' || rand() < 0.2) island.areaMm2 = rand() < 0.1 ? 0 : fp.pts.length * 0.0625 * range(0.5, 1.5);
    if (source !== 'minima' && rand() < 0.9) island.contactVoxels = footprintFromPoints(fp.pts);
    if (source === 'overhang') {
        island.overhangAngleDeg = range(0, 60);
        if (rand() < 0.5) island.dragMomentMm3 = range(0, 2000);
        if (rand() < 0.3) island.steepFlat = true;
        if (rand() < 0.7) island.maxZ = s.pos.z + range(0, 20);
        if (rand() < 0.5) island.surfaceAreaMm2 = range(10, 600);
        if (rand() < 0.8) island.surfaceNormal = rand() < 0.7 ? s.normal : unit();
        if (rand() < 0.7) island.triangleIds = trianglesUnder(scene, fp.ox, fp.oy, fp.w, fp.h);
        if (rand() < 0.5) {
            const hw = fp.w / 2 - 0.25, hh = fp.h / 2 - 0.25;
            const z = s.pos.z;
            const loop = [[fp.ox - hw, fp.oy - hh, z], [fp.ox + hw, fp.oy - hh, z + 0.3], [fp.ox + hw, fp.oy + hh, z], [fp.ox - hw, fp.oy + hh, z - 0.2]];
            island.perimeterLoops = rand() < 0.3 ? [loop, loop.map(([x, y, zz]) => [x + 0.1, y + 0.05, zz])] : [loop];
        }
    }
    return island;
}
function wireIsland(island: any): any {
    const out: any = { ...island, contact: v3(island.contact) };
    if (island.contactVoxels) {
        const f = island.contactVoxels;
        out.contactVoxels = { xy: Array.from(f.xy), z: f.z ? Array.from(f.z) : null, count: f.count };
    }
    return out;
}

// --- settings ---
function randomSettings() {
    const s: any = createDefaultSettings();
    s.autoSupport.sizingPreset = pick(['detail', 'structure', 'anchor', 'gone']);
    s.autoSupport.modelScaleEnabled = rand() < 0.8;
    s.autoSupport.modelSizeFactorCap = range(1, 2.5);
    s.autoSupport.modelLoadFactorCap = range(1, 2.5);
    s.autoSupport.heightFactorCap = range(1, 2);
    s.autoSupport.tipContactMarginScale = range(0.3, 1.2);
    setSettings(s);
    return structuredClone(getSettings());
}
function randomAuto() {
    const a: any = {
        minIslandAreaMm2: range(-0.1, 1), tipInfluenceRadiusMm: pick([0, range(0.5, 5)]), prioritizeIntersection: rand() < 0.5,
        areaPerSupportMm2: range(0.3, 30), gridAreaThresholdMm2: range(0.5, 40), overhangSelfSupportAngleDeg: range(20, 70),
        flatDensityBoost: range(0.3, 1.2), slopeRelaxFactor: range(0.8, 2), suctionAreaExponent: pick([0, range(0, 0.5)]),
        coverageTargetPercent: range(50, 100),
    };
    for (const k of Object.keys(a)) if (rand() < 0.15) delete a[k];
    return normalizeAutoSupportSettings(a);
}
const ops: unknown[] = [];
const record = (op: string, scene: string | null, settings: unknown, auto: unknown, input: unknown, result: unknown) =>
    ops.push({ op, scene, settings, auto, input, result });

// Blocker masks on some models (`modelId` = scene name).
for (const name of ['table', 'knot']) {
    const triCount = scenes[name].mesh.geometry.index!.count / 3;
    setSupportBlockedTriangles(name, Array.from({ length: Math.floor(triCount / 6) }, () => Math.floor(rand() * triCount)));
}
out.blocked = { table: null, knot: null } as Record<string, unknown>;
// (Recorded below once, from the store's own view.)

function randomCandidate(scene: Scene) {
    const s = surfacePoint(scene, rand() < 0.8);
    const c: any = {
        id: `c${Math.floor(rand() * 1e6)}`, tipPos: s.pos, tipNormal: rand() < 0.8 ? s.normal : unit(), modelId: 'm',
        source: pick(['voxel', 'minima', 'intersection', 'overhang', 'stabilization', 'reinforcement']),
        islandAreaMm2: pick([0, range(0, 0.6), range(0, 60)]), zHeight: pick([s.pos.z, range(-2, 40)]), priority: range(0, 1),
    };
    if (rand() < 0.4) c.tipDiameterMm = range(0.05, 0.8);
    if (rand() < 0.3) c.gridPoint = true;
    return c;
}

for (let i = 0; i < 600; i++) {
    const name = pick(sceneNames);
    const scene = scenes[name];
    const settings = randomSettings();
    const auto = randomAuto();
    const kind = pick(['size', 'tipCap', 'coverage', 'gen', 'dedup', 'minima', 'pose', 'grid', 'gridParts', 'budget'] as const);
    if (kind === 'size') {
        const c = randomCandidate(scene);
        const ctx = rand() < 0.2 ? undefined : {
            modelVolumeMm3: range(0, 2e5), totalCandidates: pick([0, Math.floor(range(1, 400))]),
            ...(rand() < 0.8 ? { modelSizeMm: range(0, 300) } : {}),
        };
        const sizeScale = pick([1, range(0.3, 3)]);
        record('size', name, settings, auto, { candidate: c, ctx, sizeScale }, {
            overrides: sizeParameters(c, sizeScale, ctx), factors: modelSizingFactors(ctx), preset: presetForArea(c.islandAreaMm2),
        });
    } else if (kind === 'tipCap') {
        const cs = Array.from({ length: 6 }, () => randomCandidate(scene));
        const sdf = getOrCreateSDFCache(scene.mesh);
        sdf.refreshMatrix();
        const widths = cs.map((c) => localFreeWidthMm(sdf, c.tipPos.x, c.tipPos.y, c.tipPos.z, c.tipNormal));
        const before = structuredClone(cs);
        applyContactTipCaps(cs, scene.mesh);
        record('tipCap', name, settings, auto, { candidates: before }, { widths, candidates: cs });
    } else if (kind === 'coverage') {
        const islands = Array.from({ length: 4 }, () => randomIsland(scene));
        const tips = Array.from({ length: Math.floor(range(0, 30)) }, () => {
            const isl = pick(islands);
            return { x: isl.contact.x + range(-6, 6), y: isl.contact.y + range(-6, 6), z: isl.contact.z + range(-4, 4) };
        });
        const radius = rand() < 0.5 ? undefined : range(0.5, 5);
        record('coverage', name, settings, auto, { islands: islands.map(wireIsland), tips, radius }, {
            radii: islands.map((isl) => coverageRadiusForArea(isl.areaMm2 ?? 0)),
            coverage: islands.map((isl) => computeRegionCoverage(isl, tips, radius)),
            clusters: islands.map((isl) => findUncoveredClusters(isl, tips, radius)),
            gap: buildGapFillCandidates(islands, auto, tips),
        });
    } else if (kind === 'gen') {
        const islands = Array.from({ length: Math.floor(range(0, 12)) }, () => randomIsland(scene));
        const withMesh = rand() < 0.7;
        const modelId = withMesh && rand() < 0.7 ? name : undefined;
        const prune = withMesh ? { mesh: scene.mesh, modelId } : undefined;
        record('gen', name, settings, auto, { islands: islands.map(wireIsland), withMesh, modelId: modelId ?? null }, {
            perIsland: islands.map((isl) => candidatesFromIsland(isl)),
            candidates: generateCandidates(islands, auto, prune),
        });
    } else if (kind === 'dedup') {
        const base = randomCandidate(scene);
        const cs = Array.from({ length: Math.floor(range(0, 40)) }, (_, k) => {
            const c = randomCandidate(scene);
            c.id = `d${k % 25}`; // duplicate ids on purpose: membership is by id
            c.tipPos = { x: base.tipPos.x + range(-6, 6), y: base.tipPos.y + range(-6, 6), z: base.tipPos.z + range(-7, 7) };
            c.source = pick(['overhang', 'overhang', 'voxel', 'minima']);
            if (rand() < 0.2) c.priority = base.priority;
            return c;
        });
        record('dedup', name, settings, auto, { candidates: cs }, deduplicateCandidates(cs, auto));
    } else if (kind === 'minima') {
        const islands = Array.from({ length: Math.floor(range(1, 8)) }, () => {
            const isl = randomIsland(scene);
            isl.source = rand() < 0.8 ? 'minima' : isl.source;
            isl.class = rand() < 0.8 ? 'minimaOnly' : 'intersection';
            return isl;
        });
        record('minima', name, settings, auto, { islands: islands.map(wireIsland) },
            computeMinimaReinforcementPoints(islands, scene.mesh, auto));
    } else if (kind === 'pose') {
        const positions = posedPositions(scene.mesh);
        const index = scene.mesh.geometry.index!.array as ArrayLike<number>;
        const triCount = index.length / 3;
        const rx = pick([0, range(-Math.PI, Math.PI)]), ry = pick([0, range(-Math.PI, Math.PI)]);
        const hasRaft = rand() < 0.4;
        let terms: any;
        if (rand() < 0.5) {
            terms = {
                normals: new Float64Array(triCount * 3), areas: new Float64Array(triCount),
                details: Float64Array.from({ length: triCount }, () => rand()),
                blocked: rand() < 0.5 ? null : new Set(Array.from({ length: 20 }, () => Math.floor(rand() * triCount))),
                threshold: Math.cos((range(20, 70) * Math.PI) / 180), cupCos: Math.cos((range(2, 20) * Math.PI) / 180),
            };
        }
        const s = measurePoseStability(positions, index, rx, ry, terms, hasRaft);
        record('pose', name, settings, auto, {
            rx, ry, hasRaft,
            terms: terms ? { details: Array.from(terms.details), blocked: terms.blocked ? [...terms.blocked] : null, threshold: terms.threshold, cupCos: terms.cupCos } : null,
        }, {
            posed: Array.from(positions.slice(0, 30)), stability: s, slender: isSlenderPart(s), topple: needsToppleCoverage(s),
            steep: [steepFlatNeedsCoverage(undefined, false), steepFlatNeedsCoverage(range(0, 300), rand() < 0.5)],
        });
    } else if (kind === 'grid') {
        const islands = Array.from({ length: Math.floor(range(1, 5)) }, () => {
            const isl = randomIsland(scene);
            if (rand() < 0.7) isl.source = 'overhang';
            return isl;
        });
        const withMesh = rand() < 0.8;
        const modelId = withMesh && rand() < 0.6 ? name : undefined;
        const slender = rand() < 0.3;
        const thickness = pick([0, range(0.5, 20)]);
        record('grid', name, settings, auto, { islands: islands.map(wireIsland), withMesh, modelId: modelId ?? null, slender, thickness },
            generateGridCandidates(islands, auto, withMesh ? scene.mesh : undefined, modelId, slender, thickness));
    } else if (kind === 'gridParts') {
        const isl = randomIsland(scene);
        isl.source = 'overhang';
        const pts = isl.contactVoxels ? Array.from({ length: isl.contactVoxels.count }, (_, k) => ({
            x: isl.contactVoxels.xy[k * 2], y: isl.contactVoxels.xy[k * 2 + 1], ...(isl.contactVoxels.z ? { z: isl.contactVoxels.z[k] } : {}),
        })) : [];
        const spacing = range(0.3, 6);
        const voxelAt = createVoxelSurfaceAt(pts, Math.max(spacing, 1), isl.baseZ);
        const triAt = createTriangleSurfaceAt(isl, scene.mesh);
        const probes = Array.from({ length: 12 }, () => ({ x: isl.contact.x + range(-5, 5), y: isl.contact.y + range(-5, 5) }));
        record('gridParts', name, settings, auto, { island: wireIsland(isl), spacing, probes }, {
            spacing: computeRegionSpacing(isl, auto),
            useGrid: shouldUseDensityGrid(isl, auto),
            mult: [steepFlatSpacingMultiplier(isl, spacing), steepFlatSpacingMultiplier(isl, spacing, range(0, 20))],
            bandTop: [steepFlatAnchorBandTop(isl), steepFlatAnchorBandTop(isl, true)],
            eroded: erodeFootprint(pts).length,
            boundary: buildBoundaryPoints(pts, spacing, isl.baseZ),
            voxelSamples: probes.map((p) => voxelAt(p.x, p.y)),
            triSamples: triAt ? probes.map((p) => triAt(p.x, p.y)) : null,
            ring2d: sampleBoundary2D(isl.perimeterLoops, spacing, triAt ?? voxelAt),
            perimeter: samplePerimeterLoops(isl.perimeterLoops ?? [], spacing, isl.baseZ),
        });
    } else {
        const islands = Array.from({ length: Math.floor(range(0, 10)) }, () => randomIsland(scene));
        const contacts = Array.from({ length: Math.floor(range(0, 25)) }, () => {
            const isl = islands.length > 0 ? pick(islands) : null;
            const base = isl ? isl.contact : { x: 0, y: 0, z: 0 };
            return { tip: { x: base.x + range(-3, 3), y: base.y + range(-3, 3), z: base.z + range(-2, 2) }, preset: pick(['detail', 'structure', 'anchor']) };
        });
        const input = {
            islands, contacts, areaPerSupportMm2: range(0.5, 20),
            poseDragMomentMm3: rand() < 0.3 ? undefined : range(0, 5000), toppleCoverageNeeded: rand() < 0.5,
        };
        record('budget', name, settings, auto, { ...input, islands: islands.map(wireIsland) }, computeLoadBudget(input as any));
    }
}


// Regions big enough to overflow the per-region candidate cap (stride > 1).
for (let i = 0; i < 3; i++) {
    const settings = randomSettings();
    const auto = normalizeAutoSupportSettings({ areaPerSupportMm2: 0.5, gridAreaThresholdMm2: 1, suctionAreaExponent: i === 0 ? 0 : 0.3, flatDensityBoost: 0.3 });
    const pts: Array<{ x: number; y: number; z: number }> = [];
    const side = 36 + i * 4;
    for (let a = 0; a < side * 4; a++) for (let b = 0; b < side * 4; b++) pts.push({ x: -side / 2 + a * 0.25, y: -side / 2 + b * 0.25, z: 3 + a * 0.001 });
    const island: any = {
        id: `big${i}`, source: 'overhang', contact: new THREE.Vector3(0, 0, 3), baseZ: 3, areaMm2: pts.length * 0.0625,
        overhangAngleDeg: 5, contactVoxels: footprintFromPoints(pts),
    };
    record('grid', 'knot', settings, auto, { islands: [wireIsland(island)], withMesh: false, modelId: null, slender: false, thickness: 0 },
        generateGridCandidates([island], auto, undefined, undefined, false, 0));
}

// Stabilization: every mesh, raft on and off, in its own pose.
for (const name of sceneNames) {
    for (const hasRaft of [false, true]) {
        record('stabilization', name, null, null, { hasRaft }, computeStabilizationAnchors(scenes[name].mesh, { hasRaft }));
    }
}
out.ops = ops;

// The blocker masks the store holds, so the port replays the same sets.
import { getSupportBlockedTriangles } from './src/supports/autoSupport/supportBlockers';
out.blocked = { table: [...getSupportBlockedTriangles('table')], knot: [...getSupportBlockedTriangles('knot')] };

process.stdout.write(JSON.stringify(out, (_k, v) => (v === Infinity ? 'Infinity' : v === -Infinity ? '-Infinity' : v)));
realLog('');
