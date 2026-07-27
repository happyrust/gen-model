# dbnum 8000 incremental move verification

- Project: `AvevaMarineSample`
- Element: `FTUB 24384/22403`
- Edit: E3D command `BY E 100`, followed by `SAVEWORK`
- Source session: `26 -> 27`
- Manual update result: `success`
- Changed elements: `2` (`BRAN 24384/22402`, `FTUB 24384/22403`)

| Check | Before | After | Result |
|---|---:|---:|---|
| E3D position | `W 20476 mm` | `W 20376 mm` | East `+100 mm` |
| `FTUB.POS.x` | `-20475.5` | `-20375.5` | `+100 mm` |
| World translation X | `-20475.5` | `-20375.5` | `+100 mm` |
| AABB min X | `-20497.264` | `-20397.264` | `+100 mm` |
| AABB max X | `-20405.34` | `-20305.34` | `+100 mm` |
| `pe.sesno` | `5` | `27` | Updated |
| Watermark | `26` | `27` | Updated |

The manual update preview found one pending session and two model-affecting
elements. On-demand CATA parsing completed, both generation units were reported
as `generated`, and the live test passed.

E3D's local 3D View could not be used for a viewport capture because its PML
environment reports `GPHVIEWOPT not defined`. The saved screenshots therefore
show the authoritative E3D command/position comparison; the generated model
update is verified by the regenerated `inst_relate`, world transform, and AABB.
