from typing import Any

def header(path: str) -> dict[str, Any]:
    """库文件头：db_type / dbnum / latest_sesno / latest_ses_time / file_size。"""

def is_db_file(path: str) -> bool:
    """是不是候选 AVEVA 库文件（名字形态 + 文件头两道门）。"""

def collapse_extract_files(
    entries: list[tuple[str, int, str]],
) -> dict[str, Any]:
    """同项目抽取家族归并（ADR-028）。entries 为 (project, header_dbnum, path)。

    返回 {selected: [{project, dbnum, leaf_path, parent_path}],
    shadowed_parents, duplicate_keys: [[project, dbnum], ...],
    mismatches: [{path, filename_dbnum, header_dbnum}]}。"""

def parent_gap_refno_count(leaf: str, parent: str) -> int:
    """父层索引独有、叶子没有的 refno 个数（基线只在 gap>0 时补缺）。"""

def sessions(path: str) -> list[dict[str, Any]]:
    """全部会话页（升序）：sesno / pgno / computer_name / comments / date。"""

def collect_changes(
    path: str, start: int, end: int, detail: bool = False
) -> dict[str, list[dict[str, Any]]]:
    """LEGACY 逐会话回放（纯函数）：{sesno: [op, ...]}。

    生产预览 / 执行不走这里；正式口径是 net_window。仅供对拍与逐会话取证。"""

def net_changes(
    path: str, start: int, end: int, with_noun: bool = False
) -> dict[str, Any]:
    """会话索引差分（纯函数，不查库、不逐会话解析）：窗口净增删改。

    返回 {requested_start/end, base_sesno, target_sesno,
    added/deleted/modified: [{refno, record_pgno, record_offset,
    last_touch_sesno, noun}], counts, stats}。净口径：窗口内加了又删不出现，
    删了又建判 modified。与 collect_changes（逐会话回放）互为对拍。"""

def net_window(
    path: str, start: int, end: int, detail: bool = False
) -> dict[str, Any]:
    """正式口径：解析器语义净窗口（纯文件）。与增量管线 / 手动预览同实现。

    过滤换页但内容相同的原样重写。返回 {requested_start/end,
    window: {sesno: [op, ...]}, counts, warnings, unchanged_rewrites,
    unparseable_finals}；detail=True 时 Modified 携带属性旧值/新值。
    E3D 保存期元数据（如 CACHID）仍会如实返回。"""

def element(path: str, refno: str, sesno: int | None = None) -> dict[str, Any]:
    """从文件直读单元素属性 dump；sesno 给定时读该会话或之前的最后版本。"""

def attmap(path: str, refno: str, sesno: int | None = None) -> dict[str, Any]:
    """读取生成期语义属性视图（PdmsIO 直读、合并默认与显式属性）。"""

def subtree(path: str, refno: str, sesno: int | None = None) -> dict[str, Any]:
    """通过 PdmsIO 递归直读指定元素及其全部后代（父节点优先 DFS）。"""

def generate_all_model(
    path: str, output: str, sesno: int | None = None
) -> dict[str, Any]:
    """通过 PdmsIO 直读当前索引中的全部元素并写出模型快照。"""

def generate_model(
    path: str, refno: str, output: str, sesno: int | None = None
) -> dict[str, Any]:
    """将 direct 子树元素、PdmsGeoParam、PlantMesh 和 RVM primitive 片段写成 JSON。"""

def generate_obj(
    path: str, refno: str, output: str, sesno: int | None = None
) -> dict[str, Any]:
    """将 direct 子树中的 PlantMesh 写成 OBJ 可视化产物。"""

def generate_rvm(
    path: str, refno: str, output: str, sesno: int | None = None
) -> dict[str, Any]:
    """将 direct primitive 组装为可被 rvm-rs 回读的 RVM smoke 文件。"""

def noun_dict(attlib_path: str) -> dict[str, Any]:
    """attlib 字典：{noun_count, field_count, nouns: [NounCapabilities...]}。"""
