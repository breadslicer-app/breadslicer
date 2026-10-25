// Parity oracle for block 8c of the native port: mesh to braced forest. For
// every native scan in `df_native_islands.json` (IPC form) it derives the
// islands the way `useIslands` does with the panel's defaults, then runs
// `computeAutoSupportPlan` on the scene mesh with the default settings, as the
// Auto Supports panel does (auto-bracing on, the model registered for brace
// clearance, an empty store).
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_islands_c.json
//
// Run with DF_ORACLE_DETERMINISTIC_UUID=1 run_df_script.sh; FIX points at
// df_native_islands.json.
import * as fs from 'node:fs';
import * as THREE from 'three';
import { __resetUuid } from 'uuid';
import { cloneSupportState, getSnapshot, setSnapshot } from './src/supports/state';
import { initializeBVH, accelerateGeometry } from './src/utils/bvh';
import { createDefaultSettings } from './src/supports/Settings/types';
import { setSettings, getSettings } from './src/supports/Settings/state';
import { registerMeshForAutoBrace } from './src/supports/autoBracing/meshGeometryStore';
import { computeAutoSupportPlan } from './src/supports/autoSupport/autoPlace';

console.log = () => {};
console.warn = () => {};
console.error = () => {};

const wire = (state: any) => {
    const o: any = {};
    for (const key of ['roots', 'trunks', 'branches', 'leaves', 'twigs', 'sticks', 'braces', 'stumps', 'kickstands', 'knots']) o[key] = state[key] ?? {};
    return o;
};
const wireIsland = (i: any): any => {
    const o: any = { ...i, contact: { x: i.contact.x, y: i.contact.y, z: i.contact.z } };
    if (i.contactVoxels) o.contactVoxels = { xy: Array.from(i.contactVoxels.xy), z: i.contactVoxels.z ? Array.from(i.contactVoxels.z) : null, count: i.contactVoxels.count };
    if (i.surfaceNormal) o.surfaceNormal = { x: i.surfaceNormal.x, y: i.surfaceNormal.y, z: i.surfaceNormal.z };
    if (i.members) o.members = i.members.map(wireIsland);
    return o;
};

async function main() {
    const source = fs.readFileSync('./src/volumeAnalysis/Islands/useIslands.ts', 'utf8');
    fs.writeFileSync('./src/volumeAnalysis/Islands/_useIslandsOracle.ts',
        `${source}\nexport { annotateAndCountSupports, buildIslandContactGrid };\n`);
    const hook: any = await import('./src/volumeAnalysis/Islands/_useIslandsOracle');
    const { classifyIntersection } = await import('./src/volumeAnalysis/Islands/intersection');
    const { applyFilter } = await import('./src/volumeAnalysis/Islands/filtering');

    initializeBVH();
    __resetUuid();
    const fixture = JSON.parse(fs.readFileSync(process.env.FIX!, 'utf8'));
    const meshes: Record<string, THREE.Mesh> = {};
    const normals: Record<string, number[]> = {};
    for (const [name, soup] of Object.entries(fixture.meshes) as Array<[string, number[]]>) {
        const g = new THREE.BufferGeometry();
        g.setAttribute('position', new THREE.Float32BufferAttribute(soup, 3));
        g.computeVertexNormals();
        normals[name] = Array.from(g.attributes.normal.array as Float32Array);
        accelerateGeometry(g);
        const mesh = new THREE.Mesh(g, new THREE.MeshBasicMaterial({ side: THREE.FrontSide }));
        mesh.updateMatrixWorld(true);
        meshes[name] = mesh;
        registerMeshForAutoBrace(name, g, mesh.matrixWorld);
    }

    const runs: unknown[] = [];
    fixture.runs.forEach((run: any, r: number) => {
        const layerHeightMm = run.layerHeightMm;
        // The panel's defaults (useIslands state).
        const voxelIslands = run.combined.voxelIslands
            .filter((v: any) => (v.areaMm2 ?? 0) >= 0.02)
            .map((v: any) => ({ id: v.id, source: 'voxel', contact: new THREE.Vector3(v.contact.x, v.contact.y, v.contact.z), baseZ: v.baseZ, areaMm2: v.areaMm2, layerSpan: v.layerSpan }));
        const minimaIslands = run.combined.minimaIslands.map((m: any, i: number) => ({
            id: `m${i}`, source: 'minima', contact: new THREE.Vector3(m.position.x, m.position.y, m.position.z),
            baseZ: m.position.z, vertexIndex: m.vertexIndex, seedTriangleId: m.seedTriangleId,
        }));
        const overhangIslands = run.soupScan.regions.map(hook.overhangRegionToIsland);
        const opts = { xyToleranceMm: 0.5, zBandMm: layerHeightMm };
        const proposedClassified = classifyIntersection(voxelIslands, minimaIslands, opts);
        const contouredIds: Set<string> = hook.determineContourThreshold(proposedClassified.islands, 0.05, 20);
        const finalVoxel: any[] = [];
        for (const island of voxelIslands as any[]) {
            if (island.members && island.members.length > 1 && !contouredIds.has(island.id)) finalVoxel.push(...island.members);
            else finalVoxel.push(island);
        }
        const classified = classifyIntersection(finalVoxel, minimaIslands, opts);
        const merged = hook.mergeOverhangRegions(classified.islands, overhangIslands);
        const annotated = hook.annotateAndCountSupports(merged, hook.buildIslandContactGrid(merged), [], 0, 4, layerHeightMm);
        const filtered = applyFilter(annotated.map((i: any) => ({ ...i })), { showAlreadySupported: false, showPlateContact: false });

        // The Auto Supports panel: default settings, overrides from the store.
        const settings: any = createDefaultSettings();
        setSettings(settings);
        setSnapshot(cloneSupportState(getSnapshot()));
        const empty = cloneSupportState(getSnapshot());
        const plan = computeAutoSupportPlan(filtered, run.scene, getSettings().autoSupport, empty, meshes[run.scene]);
        let result: any = null;
        if (plan) {
            const analytics: any = { ...plan.analytics };
            delete analytics.timings;
            result = { support: wire(plan.support), analytics, result: { ...plan.result, analytics: undefined } };
        }
        runs.push({ run: r, settings: structuredClone(getSettings()), islandIds: filtered.map((i: any) => i.id), result });
        process.stderr.write(`run ${r} ${run.scene}: ${filtered.length} islands, ${plan ? Object.entries(plan.result.placed).filter(([, v]) => v).map(([k, v]) => `${v}${k[0]}`).join(' ') : 'disabled'}\n`);
    });
    fs.unlinkSync('./src/volumeAnalysis/Islands/_useIslandsOracle.ts');
    process.stdout.write(JSON.stringify({ normals, runs }));
}
void main();
