// Parity oracle for block 6b of the native port: grid placement decisions
// (`decideGridPlacement`, with the stump override and host attachment), the
// cavity bridge, the forest diameter resize, the smoothed contact normal and
// the draft commits. Each "world" replays a placement sequence the way the auto
// pass does: build a trunk candidate, decide on the grid, commit the placement
// into a growing snapshot. Ids come from one counter for the whole run, so the
// port replays the same call sequence with one counter too.
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_autosupport_b.json
//
// Run with DF_ORACLE_DETERMINISTIC_UUID=1 run_df_script.sh.
import * as THREE from 'three';
import { mergeGeometries } from 'three/examples/jsm/utils/BufferGeometryUtils.js';
import { __resetUuid } from 'uuid';
import { cloneSupportState, getSnapshot } from './src/supports/state';
import { initializeBVH, accelerateGeometry } from './src/utils/bvh';
import { createDefaultSettings } from './src/supports/Settings/types';
import { setSettings, getSettings } from './src/supports/Settings/state';
import { buildTrunkData } from './src/supports/SupportTypes/Trunk/trunkBuilder';
import { buildCavityBridge } from './src/supports/SupportTypes/Trunk/useTrunkPlacement';
import { computeForestDiameterProfile, memberDiameterOf } from './src/supports/SupportTypes/Trunk/TrunkReplacement/maxConnectedDiameter';
import { decideGridPlacement } from './src/supports/PlacementLogic/Grid/gridPlacement';
import { calculateSmoothedNormal } from './src/supports/PlacementLogic/PlacementUtils';
import { draftCommitSupport } from './src/supports/autoSupport/supportDraft';
import { getSupportTypeDescriptor, SUPPORT_TYPES } from './src/supports/supportTypeRegistry';

const realLog = console.log;
console.log = () => {};
console.warn = () => {};
initializeBVH();
__resetUuid();

let seed = 0xb0b6;
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
const out: Record<string, unknown> = {};

type Scene = { mesh: THREE.Mesh; positions: number[]; normals: number[]; index: number[] | null; side: 'front' | 'back' | 'double' };
function asMesh(geometry: THREE.BufferGeometry, matrix: THREE.Matrix4 | null, side: 'front' | 'back' | 'double'): Scene {
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
const scenes: Record<string, Scene> = {
    knot: asMesh(new THREE.TorusKnotGeometry(8, 2.5, 96, 10).translate(0, 0, 14), null, 'front'),
    table: asMesh(tableGeometry(), null, 'double'),
    gap: asMesh(gapGeometry(), null, 'front'),
    low: asMesh(lowGeometry(), new THREE.Matrix4().makeRotationZ(0.3), 'front'),
};
const sceneNames = Object.keys(scenes);
out.meshes = Object.fromEntries(Object.entries(scenes).map(([name, s]) => [name, {
    positions: s.positions, normals: s.normals, index: s.index, matrix: s.mesh.matrixWorld.elements.slice(), side: s.side,
}]));

function surfacePoint(scene: Scene, wantDown: boolean) {
    const g = scene.mesh.geometry;
    const idx = g.index!;
    const pos = g.attributes.position as THREE.BufferAttribute;
    const triCount = idx.count / 3;
    for (let attempt = 0; ; attempt++) {
        const t = Math.floor(rand() * triCount);
        const v = (k: number) => new THREE.Vector3().fromBufferAttribute(pos, idx.getX(t * 3 + k)).applyMatrix4(scene.mesh.matrixWorld);
        const a = v(0), b = v(1), c = v(2);
        const n = new THREE.Triangle(a, b, c).getNormal(new THREE.Vector3());
        if (wantDown && n.z > -0.3 && attempt < 300) continue;
        let u = rand(), w = rand();
        if (u + w > 1) { u = 1 - u; w = 1 - w; }
        const p = a.clone().addScaledVector(b.clone().sub(a), u).addScaledVector(c.clone().sub(a), w);
        return { pos: v3(p), normal: v3(n) };
    }
}
function worldSettings() {
    const s: any = createDefaultSettings();
    s.grid.enabled = rand() < 0.8;
    s.grid.spacingMm = pick([2, 3, range(1.5, 6)]);
    s.grid.minBranchAngleDeg = pick([60, range(20, 75)]);
    s.grid.attachSearchStepMm = pick([2, range(0.4, 3), range(3, 9)]);
    s.shaft.diameterMm = range(0.6, 1.6);
    s.tip.lengthMm = range(1.5, 3.5);
    s.baseFlare.enabled = rand() < 0.5;
    s.autoSupport.memberHostShaftRatio = range(0, 1);
    setSettings(s);
    return structuredClone(getSettings());
}
const wire = (state: any) => {
    const o: any = {};
    for (const key of ['roots', 'trunks', 'branches', 'leaves', 'twigs', 'sticks', 'braces', 'stumps', 'kickstands', 'knots']) o[key] = state[key] ?? {};
    return o;
};

const worlds: unknown[] = [];
for (let w = 0; w < 16; w++) {
    const name = sceneNames[w % sceneNames.length];
    const scene = scenes[name];
    const settings = worldSettings();
    let snapshot = cloneSupportState(getSnapshot());
    const steps: unknown[] = [];
    const respaceAt = w % 2 === 0 ? 15 : -1;
    for (let i = 0; i < 30; i++) {
        // Half the worlds change the grid spacing mid-run: earlier roots then
        // sit between the new nodes.
        if (i === respaceAt) {
            const s: any = getSettings();
            setSettings({ ...s, grid: { ...s.grid, spacingMm: s.grid.spacingMm * pick([0.62, 1.37, 1.5]) } });
        }
        const tip = surfacePoint(scene, rand() < 0.9);
        // A cluster above earlier tips, so nodes fill and hosts carry members.
        if (i > 2 && rand() < 0.7) {
            const prev: any = pick(steps);
            tip.pos = { x: prev.tip.pos.x + range(-3, 3), y: prev.tip.pos.y + range(-3, 3), z: prev.tip.pos.z + range(0.2, 6) };
            tip.normal = rand() < 0.7 ? { x: 0, y: 0, z: -1 } : prev.tip.normal;
        }
        const withMesh = rand() < 0.75;
        const mesh = withMesh ? scene.mesh : undefined;
        const input: any = { tipPos: tip.pos, tipNormal: tip.normal, modelId: 'm' };
        const candidate = buildTrunkData({ ...input, mesh });
        const live = structuredClone(getSettings());
        const decision = decideGridPlacement({ settings: getSettings(), snapshot, candidate, tipPos: tip.pos, tipNormal: tip.normal, modelId: 'm', mesh });
        if (decision.kind === 'place') {
            snapshot = draftCommitSupport(snapshot, decision.placed.typeId, decision.placed.entity as never, decision.placed.supplied);
        }
        steps.push({ tip, withMesh, settings: live, decision });
    }
    const resized = computeForestDiameterProfile(snapshot);
    const diameters: Record<string, number> = {};
    for (const descriptor of SUPPORT_TYPES) {
        for (const entity of Object.values((resized as any)[descriptor.location.key] ?? {})) {
            diameters[(entity as any).id] = memberDiameterOf(getSupportTypeDescriptor(descriptor.id), entity as never);
        }
    }
    worlds.push({ scene: name, settings, steps, snapshot: wire(snapshot), resized: wire(resized), diameters });
}
out.worlds = worlds;

// Cavity bridges and smoothed normals, against fresh settings each.
const probes: unknown[] = [];
for (let i = 0; i < 300; i++) {
    const name = pick(sceneNames);
    const scene = scenes[name];
    const s: any = createDefaultSettings();
    s.meshToMesh.stickVsTwigCutoffMm = pick([5, range(1, 9)]);
    s.tip.contactDiameterMm = range(0.2, 0.6);
    s.shaft.diameterMm = range(0.4, 1.5);
    setSettings(s);
    const settings = structuredClone(getSettings());
    if (rand() < 0.6) {
        const tip = surfacePoint(scene, rand() < 0.85);
        const sizing = rand() < 0.5 ? undefined : { tipContactDiameterMm: range(0.15, 0.6), shaftDiameterMm: range(0.3, 1.8) };
        probes.push({ op: 'cavity', scene: name, settings, tip, sizing, result: buildCavityBridge(tip.pos, tip.normal, 'm', scene.mesh, sizing) });
    } else {
        const tip = surfacePoint(scene, false);
        const dir = new THREE.Vector3(range(-1, 1), range(-1, 1), range(-1, 1)).normalize();
        const origin = new THREE.Vector3(tip.pos.x, tip.pos.y, tip.pos.z).addScaledVector(dir, -range(1, 6));
        const raycaster = new THREE.Raycaster(origin, dir);
        const hit = raycaster.intersectObject(scene.mesh, false)[0];
        probes.push({
            op: 'smooth', scene: name, settings, origin: v3(origin), dir: v3(dir),
            result: hit ? { faceIndex: hit.faceIndex, point: v3(hit.point), normal: calculateSmoothedNormal(hit) } : null,
        });
    }
}
out.probes = probes;

process.stdout.write(JSON.stringify(out));
realLog('');
