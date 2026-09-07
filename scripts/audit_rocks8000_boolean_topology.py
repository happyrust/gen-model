"""Audit a baked boolean catalogue mesh for closed through-hole topology."""
import json
import subprocess
import sys
from collections import Counter

import numpy as np

import equi8000_live as e


phase, refno = sys.argv[1:3]
assembly_mode = '--assembly' in sys.argv[3:]
folder = e.OUT / phase
record_ref = refno.replace('/', '_')
records = json.loads((folder / 'records.json').read_text())['items']
record = next(item for item in records if item['refno'] == record_ref)
# `has_neg` marks catalogue-level cutters on the persisted record.  Design
# negatives (for example an NBOX owned by the BOX fixture) are already baked
# into the mesh but do not set that catalogue flag.
assert len(record['insts']) == 1
mesh_path = e.REPO / 'output/rocksdb-8000-20260905/meshes' / (
    record['insts'][0]['geo_hash'] + '.mesh'
)
dump = folder / f'boolean-{record_ref}-mesh.json'
subprocess.run(
    ['D:/Rust/target/debug/examples/pipe_mesh_dump.exe', str(mesh_path), str(dump)],
    check=True,
)
mesh = json.loads(dump.read_text())
vertices = np.asarray(mesh['vertices'], dtype=float)
faces = np.asarray(mesh['indices'], dtype=int).reshape(-1, 3)
# The persisted mesh deliberately duplicates vertices per triangle. Weld exact
# output coordinates before evaluating topology.
welded, inverse = np.unique(vertices, axis=0, return_inverse=True)
faces = inverse[faces]
vertices = welded
used_vertices = set(faces.reshape(-1).tolist())
edges = Counter(
    tuple(sorted((int(face[i]), int(face[(i + 1) % 3]))))
    for face in faces
    for i in range(3)
)
boundary_edges = sum(count == 1 for count in edges.values())
nonmanifold_edges = sum(count != 2 for count in edges.values())
odd_multiplicity_edges = sum(count % 2 != 0 for count in edges.values())
directed_edges = Counter(
    (int(face[i]), int(face[(i + 1) % 3]))
    for face in faces
    for i in range(3)
)
orientation_imbalanced_edges = sum(
    directed_edges[(a, b)] != directed_edges[(b, a)]
    for a, b in edges
)
chi = len(used_vertices) - len(edges) + len(faces)

adjacency = {vertex: set() for vertex in used_vertices}
for a, b in edges:
    adjacency[a].add(b)
    adjacency[b].add(a)
components = 0
remaining = set(used_vertices)
while remaining:
    components += 1
    stack = [remaining.pop()]
    while stack:
        for neighbour in adjacency[stack.pop()]:
            if neighbour in remaining:
                remaining.remove(neighbour)
                stack.append(neighbour)
genus = components - chi / 2

center = vertices.mean(axis=0)
volume = abs(sum(
    float(np.dot(vertices[a] - center, np.cross(vertices[b] - center, vertices[c] - center))) / 6
    for a, b, c in faces
))
result = {
    'refno': refno,
    'has_neg': record['has_neg'],
    'vertices': len(used_vertices),
    'faces': len(faces),
    'edges': len(edges),
    'connected_components': components,
    'euler_characteristic': chi,
    'genus': genus,
    'boundary_edges': boundary_edges,
    'nonmanifold_edges': nonmanifold_edges,
    'odd_multiplicity_edges': odd_multiplicity_edges,
    'orientation_imbalanced_edges': orientation_imbalanced_edges,
    'topology_mode': 'closed_component_assembly' if assembly_mode else 'single_holed_solid',
    'volume_mm3': volume,
    # A catalogue model may concatenate several individually closed display
    # components.  When two components meet along an exact edge, global
    # coordinate welding sees four incident faces.  That is valid for the
    # rendering assembly when every occurrence is paired with the opposite
    # direction.  The stricter single-solid mode retains the genus check used
    # by the dedicated through-hole fixture.
    'passed': (
        boundary_edges == 0
        and (
            odd_multiplicity_edges == 0 and orientation_imbalanced_edges == 0
            if assembly_mode
            else nonmanifold_edges == 0 and genus >= 1
        )
    ),
}
e.save(folder / 'boolean-topology-audit.json', result)
print(json.dumps(result))
assert result['passed']
