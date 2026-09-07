import unittest
from verify_atta_cuts import compare

class CutGateTests(unittest.TestCase):
    def test_two_cuts_matching_vertices_pass(self):
        self.assertEqual(compare({'negatives_applied':2},[[0,0,0],[1,2,3]],[[1,2,3],[0,0,0]])['symmetric_vertex_distance_mm'],0)
    def test_solid_cylinder_is_not_a_cut_pipe(self):
        with self.assertRaises(ValueError):compare({'negatives_applied':0},[[0,0,0]],[[0,0,0]])
    def test_missing_vertex_fails(self):
        with self.assertRaises(ValueError):compare({'negatives_applied':2},[[0,0,0]],[[0,0,0],[1,0,0]])
    def test_wrong_cut_position_fails(self):
        with self.assertRaises(ValueError):compare({'negatives_applied':2},[[0,0,1]],[[0,0,0]])
    def test_nan_is_not_success(self):
        with self.assertRaises(ValueError):compare({'negatives_applied':2},[[float('nan'),0,0]],[[0,0,0]])
    def test_empty_geometry_fails(self):
        with self.assertRaises(ValueError):compare({'negatives_applied':2},[],[])

if __name__ == '__main__':unittest.main()
