import unittest
from pathlib import Path
from verify import compare_tree, compare_bounds, compare_facets, compare_runtime, compare_tty_position, require_publication, require_rvm
from verify import load


class ParityTests(unittest.TestCase):
    def setUp(self):
        self.tty = 'Ref =24384/26229\nMembers\n 1 WELD 1 =24384/26230\n 2 ELBO 1 =24384/26231\n'
        self.nodes = [dict(refno='24384_26230', noun='WELD', owner='24384_26229', order=0),
                      dict(refno='24384_26231', noun='ELBO', owner='24384_26229', order=1)]

    def test_tty_member_identity_and_order_match(self):
        compare_tree(self.tty, self.nodes, '24384/26229')

    def test_tty_two_digit_noun_ordinal_may_touch_refno(self):
        tty = 'Ref =24384/26229\nMembers\n 1 BEND 10=24384/26230\n'
        compare_tree(tty, [dict(refno='24384_26230', noun='BEND',
                                owner='24384_26229', order=0)], '24384/26229')

    def test_missing_added_member_fails(self):
        with self.assertRaises(ValueError): compare_tree(self.tty, self.nodes[:1], '24384/26229')

    def test_undeleted_member_fails(self):
        with self.assertRaises(ValueError): compare_tree(self.tty.split(' 2 ELBO')[0], self.nodes, '24384/26229')

    def test_reordered_members_fail(self):
        with self.assertRaises(ValueError): compare_tree(self.tty, self.nodes[::-1], '24384/26229')

    def test_changed_owner_fails(self):
        self.nodes[1]['owner'] = '24384_90001'
        with self.assertRaises(ValueError): compare_tree(self.tty, self.nodes, '24384/26229')

    def test_duplicate_member_fails(self):
        with self.assertRaises(ValueError): compare_tree(self.tty, self.nodes + self.nodes[:1], '24384/26229')

    def test_empty_oracle_is_not_success(self):
        with self.assertRaises(ValueError): compare_tree('', [], '24384/26229')

    def test_moved_fitting_must_change_bounds(self):
        with self.assertRaises(ValueError): compare_bounds([0, 0, 334, 10, 10, 921], [0, 0, 834, 10, 10, 1421])

    def test_geometry_tolerance(self):
        compare_bounds([0, 0, 334, 10, 10, 921], [0.001, 0, 334, 10, 10, 921])

    def test_nonfinite_geometry_fails(self):
        with self.assertRaises(ValueError): compare_bounds([float('nan')]*6, [0]*6)

    def test_old_publication_is_not_increment_success(self):
        with self.assertRaises(ValueError): require_publication({'publication_status':'ready', 'published_target':{'source_end_sesno':283}}, 284)

    def test_current_publication_is_ready(self):
        require_publication({'publication_status':'ready', 'published_target':{'source_end_sesno':284}}, 284)

    def test_missing_rvm_member_fails(self):
        with self.assertRaises(ValueError): require_rvm({'only_generated':['TUBI 5'], 'only_baseline':[], 'by_noun':{} })

    def test_named_member_uses_name_not_refno(self):
        self.nodes[1]['name']='/VALVE-A'
        compare_tree(self.tty.replace('=24384/26231','/VALVE-A'), self.nodes, '24384/26229')

    def test_rvm_tube_alias_is_matched_and_extra_geometry_fails(self):
        group = dict(name='TUBE 1 of BRANCH /B', geometries=[dict(polygons_world_mm=[[[[0,0,0],[1,1,1]]]])])
        element = dict(name=None, noun='TUBI', ordinal=1, owner_desc='BRAN /B', aabb_min=[0,0,0], aabb_max=[1,1,1])
        self.assertTrue(compare_facets([group], [element])['passed'])
        self.assertFalse(compare_facets([group], [])['passed'])

    def test_runtime_missing_implied_tube_fails(self):
        item = dict(refno='24384_26229', generic='TUBI', world_aabb=dict(mins=[0,0,0], maxs=[1,1,1]))
        expected = dict(refno='24384/26229', noun='TUBI', aabb_min=[0,0,0], aabb_max=[1,1,1])
        self.assertEqual(compare_runtime({'items':[item]}, [expected]), 0)
        with self.assertRaises(ValueError): compare_runtime({'items':[item]}, [expected,expected])

    def test_rvm_duplicate_name_is_not_silently_overwritten(self):
        group = dict(name='/V', geometries=[dict(polygons_world_mm=[[[[0,0,0],[1,1,1]]]])])
        with self.assertRaises(ValueError): compare_facets([group,group], [])

    def test_tty_west_south_are_negative_coordinates(self):
        response={'attributes':[{'name':'POS','value':[-2306.1,-8259.96,921.1]}]}
        self.assertAlmostEqual(compare_tty_position('Position W 2306mm S 8260mm U 921mm', response), 0.1)

    def test_tty_position_detects_unapplied_move(self):
        response={'attributes':[{'name':'POS','value':[-2306.1,-8259.96,921.1]}]}
        with self.assertRaises(ValueError): compare_tty_position('Position W 1806mm S 8260mm U 921mm', response)


class RecordedPipeTests(unittest.TestCase):
    fixture = Path(__file__).resolve().parents[2] / 'tests/fixtures/pipe26229'

    def test_recorded_tty_tree_has_twelve_ordered_members(self):
        count = compare_tree((self.fixture/'tty-members.log').read_text(encoding='utf-8-sig'), load(self.fixture/'tree.json')['nodes'], '24384/26229')
        self.assertEqual(count, 12)

    def test_recorded_elbow_position_matches_rounded_tty(self):
        compare_tty_position((self.fixture/'tty-position.log').read_text(encoding='utf-8-sig'), load(self.fixture/'attributes-position.json'))

    def test_recorded_rvm_large_atta_differences_are_not_hidden(self):
        report = load(self.fixture/'rvm-diagnostic.json')
        by_name = {pair['key']:pair for pair in report['pairs']}
        for name in ('/Copy-of-R332.001-BRAN-01', '/Copy-of-R332.001-BRAN-02'):
            pair = by_name[name]
            with self.assertRaises(ValueError): compare_bounds(pair['generated'], pair['rvm'], tolerance=1.0)


if __name__ == '__main__': unittest.main()
