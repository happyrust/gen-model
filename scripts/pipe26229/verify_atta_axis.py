"""Regression gate for the two screenshot ATTA axes; not full mesh parity."""
import argparse
import json
from pathlib import Path
import sys


def check(generated, rvm_groups):
    expected_names = {
        '24384/26234': '/Copy-of-R332.001-BRAN-02',
        '24384/26235': '/Copy-of-R332.001-BRAN-01',
    }
    result = []
    for refno, name in expected_names.items():
        elements = [e for e in generated if e['refno'] == refno and e['noun'] == 'ATTA']
        groups = [g for g in rvm_groups if g['name'] == name]
        if len(elements) != 1 or len(groups) != 1:
            raise ValueError(f'missing or duplicate target: {refno}')
        e = elements[0]
        if e['name'] != name:
            raise ValueError(f'identity mismatch: {refno}')
        points = [v for geo in groups[0]['geometries']
                  for poly in geo['polygons_world_mm'] for ring in poly for v in ring]
        if not points:
            raise ValueError(f'empty RVM: {refno}')
        lo = [min(v[i] for v in points) for i in range(3)]
        hi = [max(v[i] for v in points) for i in range(3)]
        # These captured two cylinders are vertical. Require their two end planes
        # and lateral centres, leaving radial/tessellation differences visible.
        deltas = [abs(e['aabb_min'][2]-lo[2]), abs(e['aabb_max'][2]-hi[2])]
        deltas += [abs((e['aabb_min'][i]+e['aabb_max'][i]-lo[i]-hi[i])/2) for i in [0,1]]
        import math
        error = max(deltas)
        result.append(dict(refno=refno, axis_error_mm=error,
                           passed=all(math.isfinite(d) and d <= 0.001 for d in deltas)))
    return dict(gate='vertical_axis_and_centre_only', tolerance_mm=0.001,
                pairs=result, passed=all(r['passed'] for r in result),
                full_mesh_parity='not_asserted')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--generated', required=True)
    p.add_argument('--rvm', required=True)
    p.add_argument('--report', required=True)
    a = p.parse_args()
    report = check(json.loads(Path(a.generated).read_text(encoding='utf-8')),
                   json.loads(Path(a.rvm).read_text(encoding='utf-8')))
    Path(a.report).write_text(json.dumps(report, indent=2), encoding='utf-8')
    print(json.dumps(report))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
