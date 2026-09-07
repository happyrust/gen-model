"""Targeted ATTA cutter regression against actual RVM facet vertices, in mm."""
import argparse
import json
import math
from pathlib import Path

TARGETS = {'24384/26234': '/Copy-of-R332.001-BRAN-02', '24384/26235': '/Copy-of-R332.001-BRAN-01'}


def obj_vertices(path):
    objects = {}
    current = None
    for line in path.read_text(encoding='utf-8').splitlines():
        if line.startswith('o '):
            current = line[2:].split()[0]
            if current in objects:
                raise ValueError('duplicate OBJ object')
            objects[current] = []
        elif line.startswith('v '):
            objects[current].append(list(map(float, line.split()[1:])))
    return objects


def compare(element, generated_vertices, expected_vertices, tolerance=0.01):
    if not generated_vertices or not expected_vertices:
        raise ValueError('empty geometry')
    if any(len(v) != 3 or not all(math.isfinite(x) for x in v) for v in generated_vertices + expected_vertices):
        raise ValueError('invalid vertex')
    if element['negatives_applied'] != 2:
        raise ValueError('SCYL must apply its two child cutters')
    a = sorted(set(map(tuple, generated_vertices)))
    b = sorted(set(map(tuple, expected_vertices)))
    if len(a) != len(b):
        raise ValueError(f'vertex count mismatch: {len(a)} vs {len(b)}')
    error = max(max(min(math.dist(x,y) for y in b) for x in a),
                max(min(math.dist(x,y) for y in a) for x in b))
    if error > tolerance:
        raise ValueError(f'symmetric vertex distance {error} mm > {tolerance} mm')
    return dict(vertices=len(a), symmetric_vertex_distance_mm=error, negatives_applied=2)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--model-dir', type=Path, required=True)
    p.add_argument('--rvm', type=Path, required=True)
    p.add_argument('--report', type=Path, required=True)
    a = p.parse_args()
    elements = json.loads((a.model_dir/'elements.json').read_text(encoding='utf-8'))
    groups = json.loads(a.rvm.read_text(encoding='utf-8'))
    vertices = obj_vertices(a.model_dir/'model.obj')
    rows = []
    for refno,name in TARGETS.items():
        try:
            es = [e for e in elements if e['refno']==refno and e['name']==name and e['noun']=='ATTA']
            gs = [g for g in groups if g['name']==name]
            if len(es)!=1 or len(gs)!=1:
                raise ValueError('missing/duplicate member')
            rv = [v for geo in gs[0]['geometries'] for poly in geo['polygons_world_mm'] for ring in poly for v in ring]
            result = compare(es[0],vertices.get('element:'+refno,[]),rv)
            rows.append(dict(refno=refno,passed=True,**result))
        except ValueError as error:
            rows.append(dict(refno=refno,passed=False,error=str(error)))
    report = dict(scope='two_ATTA_cutters_and_vertex_sets_not_full_pipeline',tolerance_mm=0.01,passed=all(r['passed'] for r in rows),rows=rows)
    a.report.write_text(json.dumps(report,indent=2),encoding='utf-8')
    print(json.dumps(report))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
