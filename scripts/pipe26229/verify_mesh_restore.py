"""Compare production mesh dumps: preserve topology; tolerate only normal roundoff."""
import argparse
import json
import math
from pathlib import Path


def compare(before, after, normal_tolerance=1e-6):
    if before['vertices'] != after['vertices']:
        raise ValueError('vertex positions or ordering changed')
    if before['indices'] != after['indices']:
        raise ValueError('topology or winding changed')
    if len(before['normals']) != len(after['normals']):
        raise ValueError('normal count changed')
    worst = 0.0
    changed = 0
    for left, right in zip(before['normals'], after['normals']):
        if len(left) != 3 or len(right) != 3 or not all(math.isfinite(x) for x in left + right):
            raise ValueError('invalid normal')
        error = max(abs(x-y) for x, y in zip(left, right))
        worst = max(worst, error)
        changed += left != right
    if worst > normal_tolerance:
        raise ValueError(f'normal error {worst} exceeds {normal_tolerance}')
    return dict(geometry_exact=True, vertices=len(before['vertices']),
                indices=len(before['indices']), changed_normals=changed,
                max_normal_component_error=worst, normal_tolerance=normal_tolerance)


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('before')
    parser.add_argument('after')
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    verdict = compare(json.loads(Path(args.before).read_text()), json.loads(Path(args.after).read_text()))
    Path(args.out).write_text(json.dumps(verdict, indent=2))
    print(json.dumps(verdict))
