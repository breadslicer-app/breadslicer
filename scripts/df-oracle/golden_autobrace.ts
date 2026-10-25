// Parity oracle for block 7 of the native port: auto-bracing
// (`buildAutoBracedSnapshot`, with the Voronoi partition, pairing, kickstand
// generation, ladder patterns and reconciliation). Each run plans a forest with
// `computeAutoSupportPlan` over synthetic islands on seven meshes (the 6c
// scenes), half with auto-bracing folded into the plan; then braces the
// planned forest directly twice in a row with random bracing settings (the
// second pass regenerates or keeps the first one's braces and kickstands).
// Six models are registered for brace clearance, one is not. Distance fields
// are pooled per mesh and ids come from one counter for the whole run.
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_autobrace.json
//
// Run with DF_ORACLE_DETERMINISTIC_UUID=1 run_df_script.sh.
import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { __resetUuid } from 'uuid';
import { cloneSupportState, getSnapshot, setSnapshot } from './src/supports/state';
import { initializeBVH, accelerateGeometry } from './src/utils/bvh';
import { footprintFromPoints } from './src/volumeAnalysis/Islands/voxelFootprint';
import { createDefaultSettings } from './src/supports/Settings/types';
import { setSettings, getSettings } from './src/supports/Settings/state';
import { getRaftSettings, getRaftSettingsForModel, setRaftSettingsForModel } from './src/supports/Rafts/Crenelated/RaftState';
import { setSupportBlockedTriangles, getSupportBlockedTriangles } from './src/supports/autoSupport/supportBlockers';
import { computeAutoSupportPlan } from './src/supports/autoSupport/autoPlace';
import { buildAutoBracedSnapshot } from './src/supports/autoBracing/autoBrace';
import { registerMeshForAutoBrace } from './src/supports/autoBracing/meshGeometryStore';
import { partitionSupportsWithVoronoi } from './src/supports/autoBracing/voronoiPartitioning';

const realLog = console.log;
console.log = () => {};
console.warn = () => {};
console.error = () => {};
initializeBVH();
__resetUuid();

let seed = 0xb7ace;
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
    const v = new THREE.Vector3(range(-1, 1), range(-1, 1), range(-1, 0.2));
    if (v.lengthSq() < 1e-6) v.set(0, 0, -1);
    return v3(v.normalize());
};
const out: Record<string, unknown> = {};

// --- meshes ---
type Scene = { mesh: THREE.Mesh; positions: number[]; normals: number[]; index: number[] | null; side: 'front' | 'back' | 'double' };
function asMesh(geometry: THREE.BufferGeometry, matrix: THREE.Matrix4 | null, side: 'front' | 'back' | 'double'): Scene {
    // three-mesh-bvh builds one root per geometry group; a mesh loaded from STL
    // or 3MF has none, and neither does the port's BVH.
    geometry.clearGroups();
    if (!geometry.attributes.normal) geometry.computeVertexNormals();
    const positions = Array.from(geometry.attributes.position.array as Float32Array);
    const normals = Array.from(geometry.attributes.normal.array as Float32Array);
    const index = geometry.index ? Array.from(geometry.index.array as ArrayLike<number>) : null;
    accelerateGeometry(geometry);
    const mesh = new THREE.Mesh(geometry, new THREE.MeshBasicMaterial({
        side: side === 'front' ? THREE.FrontSide : side === 'back' ? THREE.BackSide : THREE.DoubleSide,
    }));
    if (matrix) matrix.decompose(mesh.position, mesh.quaternion, mesh.scale);
    mesh.updateMatrixWorld(true);
    return { mesh, positions, normals, index, side };
}
const posed = (euler: THREE.Euler, position: THREE.Vector3) =>
    new THREE.Matrix4().compose(position, new THREE.Quaternion().setFromEuler(euler), new THREE.Vector3(1, 1, 1));
function tableGeometry() {
    const parts = [new THREE.BoxGeometry(20, 14, 1.5).translate(0, 0, 14)];
    for (const [x, y] of [[-9, -6], [9, -6], [-9, 6], [9, 6]]) parts.push(new THREE.BoxGeometry(1.5, 1.5, 13.5).translate(x, y, 6.75));
    parts.push(new THREE.BoxGeometry(8, 6, 6).translate(-2, 1, 3));
    return mergeGeometries(parts)!;
}
function gapGeometry() {
    return mergeGeometries([
        new THREE.BoxGeometry(24, 18, 2).translate(0, 0, 1),
        new THREE.BoxGeometry(24, 18, 2).translate(0, 0, 9),
        new THREE.BoxGeometry(2, 2, 6).translate(4, -2, 5),
    ])!;
}
function lowGeometry() {
    return mergeGeometries([
        new THREE.BoxGeometry(12, 10, 1).translate(0, 0, 2),
        new THREE.BoxGeometry(1, 1, 1.5).translate(2, 1, 0.75),
        new THREE.BoxGeometry(14, 12, 1.5).translate(0, 0, 9),
    ])!;
}
function hangingCone() {
    const cone = new THREE.ConeGeometry(4, 6, 24).rotateX(Math.PI).translate(0, 0, 9);
    const slab = new THREE.BoxGeometry(14, 14, 2).translate(0, 0, 13);
    return mergeGeometries([cone.toNonIndexed(), slab.toNonIndexed()])!;
}
const scenes: Record<string, Scene> = {
    knot: asMesh(new THREE.TorusKnotGeometry(8, 2.5, 96, 10).translate(0, 0, 14), null, 'front'),
    table: asMesh(tableGeometry(), null, 'double'),
    gap: asMesh(gapGeometry(), null, 'front'),
    low: asMesh(lowGeometry(), new THREE.Matrix4().makeRotationZ(0.3), 'front'),
    cone: asMesh(hangingCone(), null, 'back'),
    cubeEdge: asMesh(new THREE.BoxGeometry(20, 20, 20), posed(new THREE.Euler(Math.PI / 4, 0, 0), new THREE.Vector3(0, 0, 14.14)), 'front'),
    plank: asMesh(new THREE.BoxGeometry(2, 30, 40).translate(0, 0, 20), posed(new THREE.Euler(0.12, 0, 0.3), new THREE.Vector3(0, 0, 0.5)), 'front'),
};
const sceneNames = Object.keys(scenes);

// Blocker masks on some models (`modelId` = scene name), and a raft under one.
for (const name of ['table', 'knot']) {
    const triCount = scenes[name].mesh.geometry.index!.count / 3;
    setSupportBlockedTriangles(name, Array.from({ length: Math.floor(triCount / 8) }, () => Math.floor(rand() * triCount)));
}
// DragonFruit's default raft is solid; half the models print without one.
for (const name of ['knot', 'gap', 'cubeEdge']) setRaftSettingsForModel(name, { ...getRaftSettings(), bottomMode: 'off' } as any);
// Brace clearance meshes: every model but `low`.
for (const [name, s] of Object.entries(scenes)) if (name !== 'low') registerMeshForAutoBrace(name, s.mesh.geometry, s.mesh.matrixWorld);
out.meshes = Object.fromEntries(Object.entries(scenes).map(([name, s]) => {
    const blocked = getSupportBlockedTriangles(name);
    return [name, {
        positions: s.positions, normals: s.normals, index: s.index, matrix: s.mesh.matrixWorld.elements.slice(), side: s.side,
        blocked: blocked.size > 0 ? [...blocked] : null, braceMesh: name !== 'low',
    }];
}));

// --- islands ---
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
function footprintAround(cx: number, cy: number, z: number, withZ: boolean) {
    const shape = pick(['rect', 'disc', 'ring', 'ell', 'line', 'speck'] as const);
    const w = shape === 'speck' ? range(0.1, 0.6) : range(0.6, 12);
    const h = shape === 'line' ? range(0.2, 1.4) : shape === 'speck' ? range(0.1, 0.6) : range(0.6, 12);
    const slopeX = range(-0.4, 0.4), slopeY = range(-0.4, 0.4);
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
    const s = surfacePoint(scene, rand() < 0.9);
    const source = pick(['voxel', 'voxel', 'minima', 'overhang', 'overhang'] as const);
    const withZ = source === 'overhang' || rand() < 0.3;
    const fp = footprintAround(s.pos.x, s.pos.y, s.pos.z, withZ);
    const island: any = {
        id: `${source[0]}${islandSerial++}`,
        source,
        contact: new THREE.Vector3(s.pos.x, s.pos.y, s.pos.z),
        baseZ: s.pos.z,
    };
    const cls = pick([undefined, 'intersection', 'voxelOnly', 'minimaOnly', 'minimaOnly'] as const);
    if (cls) island.class = cls;
    if (source !== 'minima' || rand() < 0.2) island.areaMm2 = rand() < 0.05 ? 0 : fp.pts.length * 0.0625 * range(0.5, 1.5);
    if (source !== 'minima' && rand() < 0.9) island.contactVoxels = footprintFromPoints(fp.pts);
    if (source === 'overhang') {
        island.overhangAngleDeg = range(0, 60);
        if (rand() < 0.5) island.dragMomentMm3 = range(0, 2000);
        if (rand() < 0.25) island.steepFlat = true;
        if (rand() < 0.7) island.maxZ = s.pos.z + range(0, 20);
        if (rand() < 0.5) island.surfaceAreaMm2 = range(10, 600);
        if (rand() < 0.8) island.surfaceNormal = rand() < 0.8 ? s.normal : unit();
        if (rand() < 0.6) island.triangleIds = trianglesUnder(scene, fp.ox, fp.oy, fp.w, fp.h);
        if (rand() < 0.5) {
            const hw = fp.w / 2 - 0.25, hh = fp.h / 2 - 0.25;
            const z = s.pos.z;
            const loop = [[fp.ox - hw, fp.oy - hh, z], [fp.ox + hw, fp.oy - hh, z + 0.3], [fp.ox + hw, fp.oy + hh, z], [fp.ox - hw, fp.oy + hh, z - 0.2]];
            island.perimeterLoops = [loop];
        }
    }
    return island;
}
function wireIsland(island: any): any {
    const o: any = { ...island, contact: v3(island.contact) };
    if (island.contactVoxels) {
        const f = island.contactVoxels;
        o.contactVoxels = { xy: Array.from(f.xy), z: f.z ? Array.from(f.z) : null, count: f.count };
    }
    return o;
}

// --- settings ---
function runSettings() {
    const s: any = createDefaultSettings();
    s.grid.enabled = rand() < 0.4;
    s.grid.spacingMm = pick([2, 3, range(1.5, 5)]);
    s.grid.minBranchAngleDeg = pick([45, 60, range(20, 75)]);
    s.shaft.diameterMm = range(0.6, 1.4);
    s.tip.lengthMm = range(1.5, 3);
    s.baseFlare.enabled = rand() < 0.5;
    s.autoSupport.sizingPreset = pick(['detail', 'structure', 'anchor']);
    s.autoSupport.modelScaleEnabled = rand() < 0.8;
    s.autoSupport.memberHostShaftRatio = range(0.3, 1);
    s.autoSupport.areaPerSupportMm2 = range(2, 20);
    s.autoBracing = braceSettings();
    setSettings(s);
    return structuredClone(getSettings());
}
function braceSettings(): any {
    const b: any = {
        braceDiameterMm: pick([1, range(0.4, 2.2)]),
        initialPattern: pick(['crossDiagonal', 'singleDiagonal', 'zigZag']),
        initialDistanceMm: pick([2, range(0.5, 8)]),
        repeatingPattern: pick(['crossDiagonal', 'singleDiagonal', 'zigZag']),
        patternIntervalMm: pick([8, range(2, 16)]),
        seedSpacingMm: pick([10, range(1, 30)]),
        seedJitterMm: pick([1, range(0, 6)]),
        maxBraceLengthMm: pick([10, range(3, 25)]),
        debugSectionColorsEnabled: false,
        debugVoronoiSeedsEnabled: false,
        removeExistingBracing: rand() < 0.7,
    };
    return b;
}
function runAuto(): any {
    const a: any = {
        enabled: rand() < 0.97,
        debugSkipAutoBracing: rand() < 0.5,
        stabilizationEnabled: rand() < 0.6,
        minimaReinforcementEnabled: rand() < 0.7,
        minIslandAreaMm2: range(0, 0.6),
        tipInfluenceRadiusMm: pick([0, range(0.5, 3)]),
        prioritizeIntersection: rand() < 0.5,
        maxAttachmentsPerTrunk: pick([2, 4, 8, 12]),
        areaPerSupportMm2: range(2, 20),
        gridAreaThresholdMm2: range(4, 40),
        overhangSelfSupportAngleDeg: range(30, 60),
        coverageTargetPercent: range(70, 100),
        leafFanRadiusMm: range(6, 14),
        leafFanMaxAngleDeg: range(25, 60),
        sizeScale: pick([1, range(0.7, 1.4)]),
    };
    for (const k of ['minIslandAreaMm2', 'tipInfluenceRadiusMm', 'leafFanRadiusMm', 'coverageTargetPercent']) if (rand() < 0.15) delete a[k];
    return a;
}
const wire = (state: any) => {
    const o: any = {};
    for (const key of ['roots', 'trunks', 'branches', 'leaves', 'twigs', 'sticks', 'braces', 'stumps', 'kickstands', 'knots']) o[key] = state[key] ?? {};
    return o;
};
const empty = cloneSupportState(getSnapshot());

const runs: unknown[] = [];
const lastOnScene: Record<string, any> = {};
const t0 = Date.now();
for (let r = 0; r < 90; r++) {
    const name = sceneNames[r % sceneNames.length];
    const scene = scenes[name];
    const settings = runSettings();
    const auto = runAuto();
    const islands = Array.from({ length: Math.floor(range(2, 16)) }, () => randomIsland(scene));
    const prev = lastOnScene[name];
    const chain = prev !== undefined && rand() < 0.3;
    const base = chain ? cloneSupportState(prev) : cloneSupportState(empty);
    setSnapshot(cloneSupportState(empty));
    const plan = computeAutoSupportPlan(islands, name, auto, base, scene.mesh);
    let result: any = null;
    const braces: unknown[] = [];
    if (plan) {
        const analytics: any = { ...plan.analytics };
        delete analytics.timings;
        result = { support: wire(plan.support), analytics, result: { ...plan.result, analytics: undefined } };
        setSnapshot(plan.support);
        lastOnScene[name] = cloneSupportState(getSnapshot());
        // Brace the committed forest twice in a row, the way `runAutoBracing`
        // does: a clone of the store, then the result committed back.
        for (let pass = 0; pass < 2; pass++) {
            if (pass === 1) {
                // Some kickstands and braces become the user's (no
                // `generatedBy`): kept, and braced to, instead of regenerated.
                const st: any = cloneSupportState(getSnapshot());
                for (const k of Object.values(st.kickstands) as any[]) if (rand() < 0.6) delete k.generatedBy;
                for (const b of Object.values(st.braces) as any[]) if (rand() < 0.25) delete b.generatedBy;
                setSnapshot(st);
            }
            const state = cloneSupportState(getSnapshot());
            const s = braceSettings();
            const live: any = getSettings();
            if (rand() < 0.4) setSettings({ ...live, grid: { ...live.grid, enabled: !live.grid.enabled } });
            const grid = { ...getSettings().grid };
            const built = buildAutoBracedSnapshot(state, s);
            const { snapshot: next, ...counts } = built;
            braces.push({ settings: s, grid, input: wire(state), counts, snapshot: wire(next) });
            setSnapshot(next);
        }
    }
    runs.push({
        scene: name, settings, auto, islands: islands.map(wireIsland),
        hasRaft: getRaftSettingsForModel(name).bottomMode !== 'off',
        base: chain ? wire(base) : null, result, braces,
    });
    process.stderr.write(`run ${r} ${name}: ${plan ? Object.entries(plan.result.placed).filter(([, v]) => v).map(([k, v]) => `${v}${k[0]}`).join(' ') : 'disabled'}`
        + ` | braces ${braces.map((b: any) => `${b.counts.generatedBraceCount}/${Object.keys(b.snapshot.kickstands).length}k`).join(' ')} (${Date.now() - t0}ms)
`);
}
out.runs = runs;
// The Voronoi partition on its own: random plans, settings and model ids.
const partitions: unknown[] = [];
for (let i = 0; i < 160; i++) {
    const models = pick([['m'], ['knot', 'table'], ['a-1', 'B', 'c']]);
    const cluster = rand() < 0.5;
    const nodes = Array.from({ length: Math.floor(range(1, 70)) }, (_, j) => {
        const cx = cluster ? pick([-20, 0, 25]) : 0;
        return {
            supportId: `id-${Math.floor(rand() * 400)}-${j}`,
            modelId: pick(models),
            point: { x: cx + range(-15, 15) * (rand() < 0.1 ? 0 : 1), y: range(-15, 15) },
        };
    });
    const settings = {
        seedSpacingMm: pick([10, range(0.1, 20)]),
        seedJitterMm: pick([1, range(0, 8)]),
        maxNeighborDistanceMm: pick([10, range(0.05, 15), NaN]),
    };
    partitions.push({ nodes, settings: { ...settings, maxNeighborDistanceMm: Number.isNaN(settings.maxNeighborDistanceMm) ? 'NaN' : settings.maxNeighborDistanceMm }, groups: partitionSupportsWithVoronoi(nodes, settings) });
}
out.partitions = partitions;
// localeCompare over id-like strings, for the port's collation.
const alphabet = 'aAbBzZ09-_.:';
const words = Array.from({ length: 400 }, () => Array.from({ length: 1 + Math.floor(rand() * 6) }, () => alphabet[Math.floor(rand() * alphabet.length)]).join(''));
out.collation = { words, sorted: [...words].sort((a, b) => a.localeCompare(b)) };

process.stdout.write(JSON.stringify(out));
realLog('');
