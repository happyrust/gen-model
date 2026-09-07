"""Fail-closed comparisons; no third-party Python dependencies.

TTY is the independent member oracle. RVM comparison consumes rvm_compare's
report, never treats its process exit code alone as geometric agreement.
"""
import argparse
import json
import math
from pathlib import Path
import re


def ref(value):
    return str(value).lstrip('=').replace('_', '/')


def compare_tree(tty, nodes, root):
    if not re.search(r'Ref\s*=\s*' + re.escape(ref(root)) + r'\b', tty):
        raise ValueError('TTY root identity missing or different')
    # E3D's fixed-width member listing removes the separator once the per-noun
    # ordinal reaches two digits, e.g. ``18 BEND 10=24384/22421``.
    rows = re.findall(r'^\s*(\d+)\s+([A-Z]+)\s+\d+\s*([=/][^\r\n]+)', tty, re.M)
    if not rows:
        raise ValueError('empty TTY member oracle')
    if len(rows) != len(nodes):
        raise ValueError(f'member count: TTY={len(rows)} API={len(nodes)}')
    seen = set()
    for offset, ((ordinal, noun, identity), node) in enumerate(zip(rows, nodes)):
        identity = identity.strip()
        actual = ref(node['refno'])
        if actual in seen:
            raise ValueError(f'duplicate member {actual}')
        seen.add(actual)
        expected = ref(identity) if identity.startswith('=') else identity
        observed = actual if identity.startswith('=') else node.get('name')
        if (int(ordinal), noun, expected) != (offset + 1, node['noun'], observed):
            raise ValueError(f'member/order mismatch at {offset}: {identity} / {node}')
        if node['order'] != offset or ref(node['owner']) != ref(root):
            raise ValueError(f'owner/order mismatch: {node}')
    return len(rows)


def compare_bounds(actual, expected, tolerance=0.01):
    if len(actual) != 6 or len(expected) != 6:
        raise ValueError('expected six bound coordinates')
    if not all(math.isfinite(v) for v in [*actual, *expected]):
        raise ValueError('nonfinite geometry')
    if any(v[i] > v[i+3] for v in (actual, expected) for i in range(3)):
        raise ValueError('inverted bounds')
    error = max(abs(a-b) for a, b in zip(actual, expected))
    if error > tolerance:
        raise ValueError(f'AABB error {error} mm exceeds {tolerance} mm')
    return error


def require_publication(receipt, target):
    if receipt.get('publication_status') != 'ready':
        raise ValueError('publication is not ready')
    published = receipt.get('published_target') or {}
    if published.get('source_end_sesno') != target:
        raise ValueError('publication session is stale or unpinned')


def compare_tty_position(tty, attribute_response, tolerance=0.5):
    # Q POS defaults to integer mm in this TTY. This tolerance is for the
    # independently rounded textual oracle, not for geometry comparison.
    match = re.search(r'Position\s+([EW])\s+([-\d.]+)mm\s+([NS])\s+([-\d.]+)mm\s+([UD])\s+([-\d.]+)mm', tty)
    if not match:
        raise ValueError('TTY Position missing')
    values = match.groups()
    expected = [float(values[i+1]) * (-1 if values[i] in 'WSD' else 1) for i in (0,2,4)]
    actual = next((row['value'] for row in attribute_response['attributes'] if row['name']=='POS'), None)
    if actual is None or len(actual) != 3 or not all(math.isfinite(v) for v in actual):
        raise ValueError('API POS missing/nonfinite')
    error = max(abs(a-b) for a,b in zip(actual,expected))
    if error > tolerance:
        raise ValueError(f'TTY/API position error {error} mm')
    return error


def require_rvm(report):
    for key in ('only_generated', 'only_baseline', 'unkeyable_generated', 'unkeyable_baseline'):
        if report.get(key):
            raise ValueError(f'RVM {key}: {report[key]}')
    groups = report.get('by_noun', {})
    if not groups or not sum(g.get('both', 0) for g in groups.values()):
        raise ValueError('empty RVM comparison')
    for noun, group in groups.items():
        if group.get('only_generated', 0) or group.get('only_baseline', 0) or group.get('both', 0) != group.get('exact', 0):
            raise ValueError(f'RVM non-exact {noun}: {group}')


def load(path):
    return json.loads(Path(path).read_text(encoding='utf-8-sig'))


def compare_facets(groups, elements, tolerance=0.01):
    """Compare all geometric RVM groups with generated world AABBs in mm.

    Includes named ATTA/VALV and TUBE groups. Curved tessellation differences
    are reported as failures, not silently accepted by increasing tolerance.
    """
    aliases = {'ELBO': 'ELBOW', 'FTUB': 'FTUBE', 'TUBI': 'TUBE'}
    actual = {}
    for element in elements:
        key = element.get('name')
        if not key:
            owner = element['owner_desc'].replace('BRAN ', 'BRANCH ', 1)
            key = f"{aliases.get(element['noun'], element['noun'])} {element['ordinal']} of {owner}"
        if key in actual:
            raise ValueError(f'duplicate generated key {key}')
        actual[key] = element['aabb_min'] + element['aabb_max']
    expected = {}
    for group in groups:
        if not group['geometries']:
            continue
        points = [point for geo in group['geometries']
                  for polygon in geo['polygons_world_mm'] for ring in polygon for point in ring]
        if not points:
            raise ValueError(f"empty facets: {group['name']}")
        if group['name'] in expected:
            raise ValueError(f"duplicate RVM key {group['name']}")
        expected[group['name']] = [min(p[i] for p in points) for i in range(3)] + [max(p[i] for p in points) for i in range(3)]
    pairs = []
    for key in sorted(actual.keys() & expected.keys()):
        try:
            error = compare_bounds(actual[key], expected[key], tolerance)
            pairs.append(dict(key=key, status='pass', error_mm=error))
        except ValueError as error:
            pairs.append(dict(key=key, status='fail', reason=str(error), generated=actual[key], rvm=expected[key]))
    missing = sorted(expected.keys() - actual.keys())
    extra = sorted(actual.keys() - expected.keys())
    return dict(tolerance_mm=tolerance, pairs=pairs, missing=missing, extra=extra,
                passed=bool(pairs) and not missing and not extra and all(p['status']=='pass' for p in pairs))


def compare_runtime(response, elements, tolerance=0.01):
    """Exact membership and AABB parity, grouping TUBI by root and noun.

    API currently lacks route ordinal: tube AABBs are compared as a multiset;
    this gate does not assert tube endpoint identity or tessellation topology.
    """
    if response.get('truncated') or not response.get('items'):
        raise ValueError('empty/truncated runtime records')
    actual, expected = {}, {}
    for item in response['items']:
        key = (ref(item['refno']), item['generic'])
        box = item['world_aabb']
        actual.setdefault(key, []).append(box['mins'] + box['maxs'])
    for item in elements:
        expected.setdefault((ref(item['refno']), item['noun']), []).append(item['aabb_min'] + item['aabb_max'])
    if actual.keys() != expected.keys():
        raise ValueError(f'runtime membership mismatch missing={expected.keys()-actual.keys()} extra={actual.keys()-expected.keys()}')
    worst = 0.0
    for key in expected:
        if len(actual[key]) != len(expected[key]):
            raise ValueError(f'runtime multiplicity mismatch {key}')
        for a, b in zip(sorted(actual[key]), sorted(expected[key])):
            worst = max(worst, compare_bounds(a, b, tolerance))
    return worst


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--tty', required=True)
    parser.add_argument('--tree', required=True)
    parser.add_argument('--root', default='24384/26229')
    parser.add_argument('--rvm-report')
    parser.add_argument('--receipt')
    parser.add_argument('--target', type=int)
    args = parser.parse_args()
    count = compare_tree(Path(args.tty).read_text(encoding='utf-8-sig'), load(args.tree)['nodes'], args.root)
    if args.rvm_report:
        require_rvm(load(args.rvm_report))
    if bool(args.receipt) != (args.target is not None):
        parser.error('--receipt and --target must be supplied together')
    if args.receipt:
        require_publication(load(args.receipt), args.target)
    print(json.dumps(dict(tree_members=count, tree='pass', rvm='pass' if args.rvm_report else 'not_run', publication='pass' if args.receipt else 'not_run')))


if __name__ == '__main__':
    main()
