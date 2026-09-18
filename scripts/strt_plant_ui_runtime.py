"""Prepare an isolated real plant-ui / gen-model STRT validation runtime."""
from pathlib import Path
import json
import re
import shutil

ROOT = Path(__file__).resolve().parents[1]
RUN = ROOT / 'output/strt-cpv-20260905/live'
RUN.mkdir(parents=True, exist_ok=True)
config = (ROOT / 'DbOption.toml').read_text(encoding='utf-8-sig')
# 介质不写进配置（ADR-080）：由启动服务的那一侧以 AIOS_STORE_MODE 决定，这条验证跑的是 mem（= 不设）。
overrides = {
    'project_path': 'D:/AVEVA/Projects/E3D3.1',
    'data_face': 'read-through',
    'http_api_addr': '127.0.0.1:18206',
    'meshes_path': (RUN / 'meshes').as_posix(),
    'surreal_ns': 'strt_cpv_ui_20260905',
    'watch_dbnums': [7997], 'manual_db_nums': [7997],
    'startup_autorun': False,
    'gen_model': False, 'gen_mesh': True,
    'data_incremental': False, 'model_incremental': False,
    'room_incremental': False,
}
for key, value in overrides.items():
    line = f'{key} = {json.dumps(value)}'
    pattern = rf'(?m)^{key}\s*=.*$'
    config = re.sub(pattern, lambda _: line, config) if re.search(pattern, config) else line+'\n'+config
(RUN / 'DbOption.toml').write_text(config, encoding='utf-8')
for name in ['resource', 'all_attr_info.json', 'noun_flags.json']:
    src, dst = ROOT/name, RUN/name
    if src.is_dir():
        shutil.copytree(src, dst, dirs_exist_ok=True)
    elif src.is_file():
        shutil.copy2(src, dst)
ui = RUN/'ui'
ui.mkdir(exist_ok=True)
src_ui = ROOT/'output/rocksdb-8000-20260905/ui'
shutil.copy2(src_ui/'plant-ui-app.exe', ui/'plant-ui-app.exe')
shutil.copy2(RUN/'DbOption.toml', ui/'DbOption.toml')
for name in ['assets', 'resource']:
    if (src_ui/name).is_dir():
        shutil.copytree(src_ui/name, ui/name, dirs_exist_ok=True)
(ui/'config').mkdir(exist_ok=True)
(ui/'config/settings.ron').write_text(
    '(theme: Light, density: Standard, model_api_url: "http://127.0.0.1:18206", '
    f'mesh_dir: "{(RUN / "meshes").as_posix()}")', encoding='utf-8')
print(RUN)
