// Parity oracle for block 8b of the native port: the island derivation
// `useIslands` runs between the native scans and auto-support. Reads the
// native scan fixture (`df_native_islands.json`, in the IPC form the frontend
// receives) and, per scan, runs the hook's chain with DragonFruit's own
// functions: voxel/minima mapping, overhangRegionToIsland, optional voxel
// consolidation, classifyIntersection, determineContourThreshold and the
// member revert, mergeOverhangRegions, the contact grid and support
// annotation, applyFilter and scanNeedsCoverage. Panel settings and existing
// support tips vary per case.
//
// `annotateAndCountSupports` and `buildIslandContactGrid` are module-private,
// so the script writes a sandbox copy of useIslands.ts with one export line
// appended; nothing else in it changes.
// Output: libs/sc-geom/support-gen-df/tests/fixtures/df_islands_b.json
//
// Run with run_df_script.sh; FIX points at df_native_islands.json.
import * as fs from 'node:fs';
import * as THREE from 'three';
import './src/supports/state';

console.log = () => {};
console.warn = () => {};

async function main() {
const source = fs.readFileSync('./src/volumeAnalysis/Islands/useIslands.ts', 'utf8');
fs.writeFileSync(
    './src/volumeAnalysis/Islands/_useIslandsOracle.ts',
    `${source}\nexport { annotateAndCountSupports, buildIslandContactGrid };\n`,
);
const hook: any = await import('./src/volumeAnalysis/Islands/_useIslandsOracle');
const { classifyIntersection } = await import('./src/volumeAnalysis/Islands/intersection');
const { applyFilter } = await import('./src/volumeAnalysis/Islands/filtering');

let seed = 0x8b8b;
function rand(): number {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const range = (lo: number, hi: number) => lo + (hi - lo) * rand();
const pick = <T>(xs: readonly T[]): T => xs[Math.floor(rand() * xs.length)];

/** A footprint as its count and an FNV-1a over its f32 bits (xy, then z). */
const digest = (f: any) => {
    let h = 2166136261;
    const eat = (a: Float32Array) => {
        const u = new Uint32Array(a.buffer, a.byteOffset, a.length);
        for (let i = 0; i < u.length; i++) { h ^= u[i]; h = Math.imul(h, 16777619); }
    };
    eat(f.xy.subarray(0, f.count * 2));
    if (f.z) eat(f.z.subarray(0, f.count));
    return { count: f.count, withZ: f.z !== null, fnv: h >>> 0 };
};
const wireIsland = (i: any): any => {
    const o: any = { ...i, contact: { x: i.contact.x, y: i.contact.y, z: i.contact.z } };
    if (i.contactVoxels) o.contactVoxels = digest(i.contactVoxels);
    if (i.members) o.members = i.members.map(wireIsland);
    if (i.surfaceNormal) o.surfaceNormal = { x: i.surfaceNormal.x, y: i.surfaceNormal.y, z: i.surfaceNormal.z };
    return o;
};

const fixture = JSON.parse(fs.readFileSync(process.env.FIX!, 'utf8'));
const cases: unknown[] = [];
fixture.runs.forEach((run: any, r: number) => {
    const layerHeightMm = run.layerHeightMm;
    for (let variant = 0; variant < 3; variant++) {
        const settings = {
            pxMm: variant === 0 ? 0.05 : pick([0.05, range(0.02, 0.3)]),
            minAreaMm2: variant === 0 ? 0.02 : pick([0.02, range(0, 1.5)]),
            consolidateVoxel: variant === 0 ? false : rand() < 0.6,
            consolidationDistance: variant === 0 ? 0.2 : pick([0.2, range(0.05, 4)]),
            enableContourRegions: variant === 0 ? true : rand() < 0.7,
            maxContourRegions: variant === 0 ? 20 : pick([20, Math.floor(range(0, 8))]),
            areaPerSupport: variant === 0 ? 4 : pick([4, range(0.5, 20)]),
            plateZ: variant === 0 ? 0 : pick([0, range(-0.5, 2)]),
            filterToggles: variant === 0
                ? { showAlreadySupported: false, showPlateContact: false }
                : { showAlreadySupported: rand() < 0.3, showPlateContact: rand() < 0.3 },
        };

        // The hook's own mappings of the combined scan.
        const combined = run.combined;
        const voxelIslands = combined.voxelIslands
            .filter((v: any) => (v.areaMm2 ?? 0) >= settings.minAreaMm2)
            .map((v: any) => ({
                id: v.id,
                source: 'voxel',
                contact: new THREE.Vector3(v.contact.x, v.contact.y, v.contact.z),
                baseZ: v.baseZ,
                areaMm2: v.areaMm2,
                layerSpan: v.layerSpan,
            }));
        const minimaIslands = combined.minimaIslands.map((m: any, i: number) => ({
            id: `m${i}`,
            source: 'minima',
            contact: new THREE.Vector3(m.position.x, m.position.y, m.position.z),
            baseZ: m.position.z,
            vertexIndex: m.vertexIndex,
            seedTriangleId: m.seedTriangleId,
        }));
        const overhangIslands = run.soupScan.regions.map(hook.overhangRegionToIsland);

        // Existing tips: some on island contacts, some anywhere near the model.
        const anchors = [...voxelIslands, ...minimaIslands, ...overhangIslands];
        const supportTips = variant === 0 ? [] : Array.from({ length: Math.floor(range(0, 10)) }, () => {
            const a = anchors.length > 0 && rand() < 0.7 ? pick(anchors) : null;
            const pos = a
                ? new THREE.Vector3(a.contact.x + range(-0.4, 0.4), a.contact.y + range(-0.4, 0.4), a.contact.z + pick([0, range(-0.3, 0.3), range(-1, 1)]))
                : new THREE.Vector3(range(-20, 20), range(-20, 20), range(0, 30));
            return { pos, diameterMm: pick([0.4, range(0.2, 1.2)]) };
        });

        const opts = { xyToleranceMm: 0.5, zBandMm: layerHeightMm };
        const proposedConsolidated = settings.consolidateVoxel
            ? hook.consolidateVoxelIslands(voxelIslands, settings.consolidationDistance, settings.pxMm)
            : voxelIslands;
        const proposedClassified = classifyIntersection(proposedConsolidated, minimaIslands, opts);
        const contouredIds: Set<string> = settings.enableContourRegions
            ? hook.determineContourThreshold(proposedClassified.islands, settings.pxMm, settings.maxContourRegions)
            : new Set<string>();
        const finalVoxelIslands: any[] = [];
        for (const island of proposedConsolidated) {
            if (island.members && island.members.length > 1 && !contouredIds.has(island.id)) finalVoxelIslands.push(...island.members);
            else finalVoxelIslands.push(island);
        }
        const classifiedResult = classifyIntersection(finalVoxelIslands, minimaIslands, opts);
        const merged = hook.mergeOverhangRegions(classifiedResult.islands, overhangIslands);
        const grid = hook.buildIslandContactGrid(merged);
        const annotated = hook.annotateAndCountSupports(merged, grid, supportTips, settings.plateZ, settings.areaPerSupport, layerHeightMm);
        const filtered = applyFilter(annotated.map((i: any) => ({ ...i })), settings.filterToggles);

        cases.push({
            run: r,
            settings,
            supportTips: supportTips.map((t) => ({ pos: { x: t.pos.x, y: t.pos.y, z: t.pos.z }, diameterMm: t.diameterMm })),
            result: {
                // The filter keeps annotated order, so its ids say it all.
                filtered: filtered.map((i: any) => i.id),
                annotated: annotated.map(wireIsland),
                stats: classifiedResult.stats,
                toppleCoverage: hook.scanNeedsCoverage(run.soupScan.stability),
                dragTotalMm3: run.soupScan.stability?.dragMomentMm3 ?? 0,
            },
        });
    }
});
// consolidateVoxelIslands and determineContourThreshold on islands that carry
// footprints (native voxel islands carry none, so the chain above never
// dilates or contours): clustered synthetic islands, 0.05–0.25 mm voxels.
const { footprintFromPoints } = await import('./src/volumeAnalysis/Islands/voxelFootprint');
const direct: unknown[] = [];
for (let k = 0; k < 120; k++) {
    const px = pick([0.05, 0.1, range(0.03, 0.25)]);
    const n = Math.floor(range(1, 30));
    const centres = Array.from({ length: Math.floor(range(1, 5)) }, () => ({ x: range(-10, 10), y: range(-10, 10), z: range(0, 20) }));
    const islands = Array.from({ length: n }, (_, i) => {
        const c = pick(centres);
        const x = c.x + range(-1.5, 1.5), y = c.y + range(-1.5, 1.5), z = c.z + range(-0.3, 0.3);
        const w = Math.floor(range(0, 9));
        const pts = [];
        for (let a = 0; a < w; a++) for (let b = 0; b < Math.floor(range(1, 6)); b++) pts.push({ x: x + a * px, y: y + b * px });
        const island: any = {
            id: `v${i}`, source: 'voxel', contact: new THREE.Vector3(x, y, z), baseZ: z,
            areaMm2: pick([pts.length * px * px, range(0, 0.2), range(0, 5)]),
        };
        if (rand() < 0.8) island.layerSpan = [Math.floor(z / 0.05), Math.floor(z / 0.05) + Math.floor(range(0, 30))];
        if (pts.length > 0 && rand() < 0.85) island.contactVoxels = footprintFromPoints(pts);
        island.class = pick(['voxelOnly', 'intersection', 'minimaOnly', undefined]);
        return island;
    });
    const eps = pick([0.2, range(0.05, 3)]);
    const maxContour = pick([20, Math.floor(range(0, 12))]);
    direct.push({
        px, eps, maxContour,
        islands: islands.map((i: any) => ({ ...wireIsland(i), contactVoxels: i.contactVoxels ? { xy: Array.from(i.contactVoxels.xy), z: i.contactVoxels.z ? Array.from(i.contactVoxels.z) : null, count: i.contactVoxels.count } : undefined })),
        consolidated: hook.consolidateVoxelIslands(islands, eps, px).map(wireIsland),
        contoured: [...hook.determineContourThreshold(islands, px, maxContour)].sort(),
    });
}
fs.unlinkSync('./src/volumeAnalysis/Islands/_useIslandsOracle.ts');
process.stdout.write(JSON.stringify({ cases, direct }));
}
void main();
