# 24384/26229 diagnostic recording

Recorded read-only on 2026-09-05 from the screenshot branch in AMS /ALL, DB8000 session 285.
Original capture: `output/pipe26229-baseline-20260905`.

- tty-members.log: actual E3D Q MEMB output, 12 members.
- tree.json: actual /api/v1/tree/children response.
- tty-position.log: actual Q REF / Q POS / Q ORI for 24384/26239.
- attributes-position.json: actual API response metadata, retaining only POS/ORI/OWNER/SPRE/LSTU rows.
- rvm-diagnostic.json: actual world-AABB comparison of 16 generated geometries with 16 freshly exported RVM groups.

This is a diagnostic fixture, NOT a certified golden geometry baseline. Full capture detected source-file
hash changes while the session remained 285. Two ATTA groups have large differences; other curved groups
also exceed the strict 0.01 mm tessellated-world-AABB tolerance. Tests assert that these mismatches remain
visible, not that mismatching generated geometry is correct. No add/delete/move SAVEWORK was executed here.
