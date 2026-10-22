# SPDX-License-Identifier: Apache-2.0
"""Independent ZIP/XML geometry comparison for our trusted synthetic 3MF corpus.

Supports the one-model-part fixtures used by SC-14, not the full 3MF vocabulary.
"""
from pathlib import Path
import hashlib,json,zipfile,xml.etree.ElementTree as ET,struct

NS={'m':'http://schemas.microsoft.com/3dmanufacturing/core/2015/02'}

def transform(p,raw):
    if not raw:return p
    a=list(map(float,raw.split()))
    assert len(a)==12
    return tuple(sum(p[j]*a[j*3+i] for j in range(3))+a[9+i] for i in range(3))

def signature(triangles):
    # Cyclic normalization preserves orientation but ignores triangle/vertex IDs.
    canonical=[]
    for tri in triangles:
        pts=[tuple(round(v,5)+0.0 for v in p) for p in tri]
        canonical.append(min(tuple(pts[i:]+pts[:i]) for i in range(3)))
    return hashlib.sha256(json.dumps(sorted(canonical),separators=(',',':')).encode()).hexdigest()

def inspect(path):
    path=Path(path)
    with zipfile.ZipFile(path) as z:
        models=[n for n in z.namelist() if n.endswith('.model')]
        assert len(models)==1
        root=ET.fromstring(z.read(models[0]))
        unit=root.get('unit','millimeter');factor={'millimeter':1.,'inch':25.4,'centimeter':10.}[unit]
        objects={o.get('id'):o for o in root.findall('m:resources/m:object',NS)}
        leaves=[]
        def visit(oid,transforms):
            obj=objects[oid];mesh=obj.find('m:mesh',NS)
            if mesh is not None:
                vertices=[tuple(float(v.get(axis)) for axis in ['x','y','z']) for v in mesh.find('m:vertices',NS)]
                triangles=[]
                for t in mesh.find('m:triangles',NS):
                    tri=[]
                    for k in ['v1','v2','v3']:
                        p=vertices[int(t.get(k))]
                        for tr in transforms:p=transform(p,tr)
                        tri.append(tuple(x*factor for x in p))
                    triangles.append(tri)
                pts=[p for t in triangles for p in t]
                lo=[min(p[i] for p in pts) for i in range(3)];hi=[max(p[i] for p in pts) for i in range(3)]
                # Center-normalized signature checks shape independently of placement.
                centered=[[[p[i]-lo[i] for i in range(3)] for p in t] for t in triangles]
                leaves.append({'name':obj.get('name'),'triangles':len(triangles),'bounds_mm':[lo,hi],
                    'geometry_hash':signature(triangles),'shape_hash':signature(centered),'transforms':transforms})
            else:
                for c in obj.findall('m:components/m:component',NS):visit(c.get('objectid'),[c.get('transform')]+transforms)
        for item in root.findall('m:build/m:item',NS):visit(item.get('objectid'),[item.get('transform')])
        payload={n:z.read(n).decode('utf-8') for n in z.namelist() if n.endswith('.toml')}
        return {'file':path.name,'sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'leaves':leaves,'toml_parts':payload}

if __name__=='__main__':
    import sys
    results=[inspect(p) for p in sys.argv[1:]]
    print(json.dumps(results,indent=2,ensure_ascii=False))
