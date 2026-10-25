"""Write posed binary STLs for the DF oracle corpus (Z up, build-plate mm).

usage: pose_stl.py stl IN OUT --rx DEG --ry DEG --center --minz MM
       pose_stl.py 3mf IN OUT            (applies the 3MF build-item transform)
"""
import math, re, struct, sys, zipfile
from array import array


def read_stl(path):
    data = open(path, 'rb').read()
    n = struct.unpack_from('<I', data, 80)[0]
    if 84 + 50 * n != len(data):
        raise SystemExit(f'{path}: not a binary STL')
    v = array('f')
    for i in range(n):
        v.frombytes(data[84 + 50 * i + 12: 84 + 50 * i + 48])
    return v  # 9 floats per triangle


def read_3mf(path):
    z = zipfile.ZipFile(path)
    text = z.read('3D/3dmodel.model').decode()
    num = r'(-?[0-9.eE+-]+)'
    verts = [tuple(map(float, m)) for m in re.findall(
        rf'<vertex x="{num}" y="{num}" z="{num}"', text)]
    tris = re.findall(r'<triangle v1="(\d+)" v2="(\d+)" v3="(\d+)"', text)
    t = re.search(r'transform="([^"]+)"', text)
    m = list(map(float, t.group(1).split())) if t else [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]
    out = array('f')
    for tri in tris:
        for idx in tri:
            x, y, zz = verts[int(idx)]
            # 3MF: row vector times 4x3 matrix (m00 m01 m02 m10 ... m30 m31 m32).
            out.extend((x * m[0] + y * m[3] + zz * m[6] + m[9],
                        x * m[1] + y * m[4] + zz * m[7] + m[10],
                        x * m[2] + y * m[5] + zz * m[8] + m[11]))
    return out


def rotate(v, axis, deg):
    c, s = math.cos(math.radians(deg)), math.sin(math.radians(deg))
    for i in range(0, len(v), 3):
        x, y, z = v[i], v[i + 1], v[i + 2]
        if axis == 'x':
            v[i + 1], v[i + 2] = y * c - z * s, y * s + z * c
        else:
            v[i], v[i + 2] = x * c + z * s, -x * s + z * c


def write_stl(path, v):
    n = len(v) // 9
    with open(path, 'wb') as f:
        f.write(b'posed for DF oracle'.ljust(80, b' '))
        f.write(struct.pack('<I', n))
        for t in range(n):
            f.write(b'\0' * 12)
            f.write(v[9 * t: 9 * t + 9].tobytes())
            f.write(b'\0\0')


def main(a):
    kind, src, dst = a[0], a[1], a[2]
    v = read_stl(src) if kind == 'stl' else read_3mf(src)
    opts = a[3:]
    if '--rx' in opts: rotate(v, 'x', float(opts[opts.index('--rx') + 1]))
    if '--ry' in opts: rotate(v, 'y', float(opts[opts.index('--ry') + 1]))
    xs, ys, zs = v[0::3], v[1::3], v[2::3]
    dx = -(min(xs) + max(xs)) / 2 if '--center' in opts else 0.0
    dy = -(min(ys) + max(ys)) / 2 if '--center' in opts else 0.0
    dz = float(opts[opts.index('--minz') + 1]) - min(zs) if '--minz' in opts else 0.0
    for i in range(0, len(v), 3):
        v[i] += dx; v[i + 1] += dy; v[i + 2] += dz
    write_stl(dst, v)
    xs, ys, zs = v[0::3], v[1::3], v[2::3]
    print(f'{dst}: {len(v)//9} tris, x[{min(xs):.2f},{max(xs):.2f}] '
          f'y[{min(ys):.2f},{max(ys):.2f}] z[{min(zs):.2f},{max(zs):.2f}]')


main(sys.argv[1:])
