import copy
import unittest
from verify_mesh_restore import compare


class MeshRestoreTests(unittest.TestCase):
    def setUp(self):
        self.mesh = dict(vertices=[[0, 0, 0], [1, 0, 0], [0, 1, 0]],
                         indices=[0, 1, 2], normals=[[0, 0, 1]]*3)

    def test_only_normal_roundoff_is_allowed(self):
        after = copy.deepcopy(self.mesh)
        after['normals'][0] = [6.2e-15, 0, 1]
        self.assertTrue(compare(self.mesh, after)['geometry_exact'])

    def test_changed_vertex_fails(self):
        after = copy.deepcopy(self.mesh)
        after['vertices'][0][0] = 0.001
        with self.assertRaises(ValueError): compare(self.mesh, after)

    def test_reversed_winding_fails(self):
        after = copy.deepcopy(self.mesh)
        after['indices'] = [0, 2, 1]
        with self.assertRaises(ValueError): compare(self.mesh, after)

    def test_changed_or_invalid_normals_fail(self):
        for value in [0.001, float('nan')]:
            after = copy.deepcopy(self.mesh)
            after['normals'][0] = [value, 0, 1]
            with self.assertRaises(ValueError): compare(self.mesh, after)


if __name__ == '__main__': unittest.main()
