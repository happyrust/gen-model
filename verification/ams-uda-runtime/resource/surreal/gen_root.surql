-- 生成根覆盖：把「哪些元素是全量生成的入口」变成图上的一等数据。
--
-- 生成接口只按「根」工作（on_demand_model.rs 会先把任意 refno 归一到根再整根重跑），
-- 所以全量生成必须先枚举出一组根。两类根的子树互不重叠、并集覆盖全部非容器存活元素：
--   mdu     —— 最外层最小交付单元（BRAN/EQUI/HANG/SUPPO 且无 MDU 祖先）
--   residue —— 不属于任何 MDU 的部分，取「子树不含 MDU 的最高节点」
-- 容器 WORL/SITE/ZONE 永不成根，与 generation_root.rs 的 COARSE_HIERARCHY_NOUNS 一致。
--
-- 本文件只定义函数与索引，开机随目录扫描重复加载无副作用；物化要显式调 fn::sync_gen_roots。

DEFINE INDEX IF NOT EXISTS gen_root_dbnum ON gen_root FIELDS dbnum;
DEFINE INDEX IF NOT EXISTS gen_root_dbnum_status ON gen_root FIELDS dbnum, status;
DEFINE INDEX IF NOT EXISTS gen_root_pe ON gen_root FIELDS pe UNIQUE;

REMOVE FUNCTION IF EXISTS fn::gen_root_mdu_types;
DEFINE FUNCTION fn::gen_root_mdu_types() {
    return ['BRAN', 'EQUI', 'HANG', 'SUPPO'];
};

REMOVE FUNCTION IF EXISTS fn::gen_root_container_types;
DEFINE FUNCTION fn::gen_root_container_types() {
    return ['WORL', 'WORLD', 'SITE', 'ZONE'];
};

-- loop / 顶点 / 基准这类只描述坐标的 noun，服务端从设计上就拒绝拿它们当根：
-- generation_root.rs:233 的兜底是 (!is_loop_container_noun(self)).then(...)，所以哪怕
-- 向上无路可走也不会退让。正常情况它们会上溯到非容器 owner（PANE/EXTR/WALL）再生成，
-- 但断头节点没有 owner 可上溯，把它们排进根集只会换来一串 422。
--
-- 这份清单对齐 is_loop_container_noun 的三个来源：pdms_types 的 TOTAL_LOOP_NOUN_NAMES
-- 与 TOTAL_VERT_NOUN_NAMES、函数里硬编码的 *DATU，以及 dabacon 字典 point 能力位命中的
-- SPINE。7997 全量 sweep 实测的 46 个 422 与本清单逐个吻合。
REMOVE FUNCTION IF EXISTS fn::gen_root_pointish_types;
DEFINE FUNCTION fn::gen_root_pointish_types() {
    return ['LOOP', 'PLOO', 'VERT', 'PAVE', 'JLDATU', 'PLDATU', 'ENDATU', 'SPINE'];
};

-- 算出一个 dbnum 的完整根覆盖，返回 { mdu: [...], residue: [...] }。
REMOVE FUNCTION IF EXISTS fn::gen_root_cover;
DEFINE FUNCTION fn::gen_root_cover($dbnum: number) {
    let $mdu = fn::gen_root_mdu_types();
    let $ctr = fn::gen_root_container_types();
    let $skip = array::union($ctr, fn::gen_root_pointish_types());
    let $alive = (select value id from pe where dbnum = $dbnum and deleted != true);

    -- 32 跳的上界对齐 generation_root.rs 的 MAX_ANCESTOR_DEPTH；向上递归不能写无界
    -- {..}，实测那样只走一跳。
    let $mdu_roots = (select value id from $alive
        where noun in $mdu
          and array::len(array::intersect(id.{..32+collect}->pe_owner->pe.noun, $mdu)) == 0);

    -- 承载链 = MDU 根的全部祖先。这些节点的子树里有 MDU，整体成根会和 MDU 根重叠。
    let $mdu_anc = array::distinct(array::flatten(
        (select value id.{..32+collect}->pe_owner->pe from $mdu_roots)));

    -- 「承载链 ∪ 容器」的直接子节点里，自身非容器、非 MDU、也不在承载链上的那些，
    -- 恰好就是「子树不含 MDU 的最高节点」。一次局部判定即可，不必自底向上标注整棵树。
    let $parents = array::distinct(array::union($mdu_anc,
        (select value id from $alive where noun in $ctr)));
    let $kids = array::distinct(array::flatten(
        (select value <-pe_owner<-pe from $parents)));
    let $residue_kids = (select value id from $kids
        where deleted != true
          and noun not in $skip
          and noun not in $mdu
          and id not in $mdu_anc);

    -- 断头节点：owner 指向的记录不存在，图上没有父边，自成一个连通分量。7997 上有 177
    -- 个（PANE/PAVE/CWALL 等），漏掉它们会少覆盖近两万个元素。
    let $orphans = (select value id from $alive
        where array::len(id->pe_owner) == 0
          and noun not in $skip
          and noun not in $mdu);

    return {
        mdu: $mdu_roots,
        residue: array::distinct(array::union($residue_kids, $orphans))
    };
};

-- 把根覆盖物化进 gen_root 表。只写身份字段，不碰 status 等进度字段，所以可以随时重算
-- 而不丢已完成的生成记录；不再成为根的行会被清掉，它们的进度已经没有意义。
REMOVE FUNCTION IF EXISTS fn::sync_gen_roots;
DEFINE FUNCTION fn::sync_gen_roots($dbnum: number) {
    let $cover = fn::gen_root_cover($dbnum);
    for $r in $cover.mdu {
        upsert type::thing('gen_root', record::id($r)) set
            pe = $r,
            dbnum = $dbnum,
            kind = 'mdu',
            noun = $r.noun,
            subtree = 1 + array::len($r.{..+collect}<-pe_owner<-pe),
            computed_at = time::now();
    };
    for $r in $cover.residue {
        upsert type::thing('gen_root', record::id($r)) set
            pe = $r,
            dbnum = $dbnum,
            kind = 'residue',
            noun = $r.noun,
            subtree = 1 + array::len($r.{..+collect}<-pe_owner<-pe),
            computed_at = time::now();
    };
    let $keep = array::union($cover.mdu, $cover.residue);
    delete gen_root where dbnum = $dbnum and pe not in $keep;
    return {
        dbnum: $dbnum,
        mdu: array::len($cover.mdu),
        residue: array::len($cover.residue),
        total: array::len($keep)
    };
};

-- root_covers 反向索引：gen_root -> 子树全部成员（含根自身）。
--
-- 建了它之后「任意 refno 属于哪个根」是一跳：pe:X<-root_covers<-gen_root，不必再向上走
-- 最多 32 级祖先。代价是全量约 15.6 万条边，且树一变就要重建对应根的那一片。
REMOVE FUNCTION IF EXISTS fn::sync_root_covers;
DEFINE FUNCTION fn::sync_root_covers($dbnum: number, $roots: option<array>) {
    let $targets = $roots ?? (select value pe from gen_root where dbnum = $dbnum);
    for $r in $targets {
        let $g = type::thing('gen_root', record::id($r));
        let $members = array::union([$r], $r.{..+collect}<-pe_owner<-pe);
        delete $g->root_covers;
        relate $g->root_covers->$members;
    };
    return {
        dbnum: $dbnum,
        roots: array::len($targets),
        edges: (select count() as c from root_covers group all)[0].c
    };
};

-- 任意元素归一到它的生成根。有 root_covers 就一跳，没有就退回走祖先链。
REMOVE FUNCTION IF EXISTS fn::gen_root_of;
DEFINE FUNCTION fn::gen_root_of($pe: record) {
    let $hit = ($pe<-root_covers<-gen_root)[0];
    if $hit != none { return $hit; };
    let $chain = array::union([$pe], $pe.{..32+collect}->pe_owner->pe);
    return (select value id from gen_root where pe in $chain limit 1)[0];
};

-- 生成成功的三个状态。
REMOVE FUNCTION IF EXISTS fn::gen_root_done_status;
DEFINE FUNCTION fn::gen_root_done_status() {
    return ['Generated', 'AlreadyAvailable', 'NoRenderableGeometry'];
};

-- 不该再重跑的状态。除了三个成功态，还有 http_422——那是服务端判定该构件解析不出合法
-- 生成根（UnresolvableRoot::NoRoot），属 noun 与数据的永久性质，重试多少次都一样。
-- 正常同步过 gen_root 之后不该再出现 422；留在终态里是为了兼容旧数据。
REMOVE FUNCTION IF EXISTS fn::gen_root_terminal_status;
DEFINE FUNCTION fn::gen_root_terminal_status() {
    return array::union(fn::gen_root_done_status(), ['http_422']);
};

-- 续跑清单：还没落到终态的根，大子树优先。取代「CSV 减去 ndjson 已完成集」那套差集。
REMOVE FUNCTION IF EXISTS fn::gen_roots_todo;
DEFINE FUNCTION fn::gen_roots_todo($dbnum: number) {
    let $terminal = fn::gen_root_terminal_status();
    let $rows = (select pe, subtree from gen_root
        where dbnum = $dbnum and status not in $terminal
        order by subtree desc);
    return $rows.pe;
};

-- 回报一个根的生成结果。$rec 缺的键会写成 NONE，语义与 ndjson 那一行一致。
REMOVE FUNCTION IF EXISTS fn::gen_root_report;
DEFINE FUNCTION fn::gen_root_report($pe: record, $rec: object) {
    upsert type::thing('gen_root', record::id($pe)) set
        status = $rec.status,
        renderable = $rec.renderable,
        written = $rec.written,
        ms = $rec.ms,
        attempt = $rec.attempt,
        error = $rec.error,
        updated_at = time::now();
};

-- 进度总览，取代对 ndjson 的聚合统计。
REMOVE FUNCTION IF EXISTS fn::gen_root_progress;
DEFINE FUNCTION fn::gen_root_progress($dbnum: number) {
    let $rows = (select kind, status, subtree from gen_root where dbnum = $dbnum);
    let $done = $rows[where status in fn::gen_root_done_status()];
    let $terminal = $rows[where status in fn::gen_root_terminal_status()];
    return {
        total: array::len($rows),
        done: array::len($done),
        refused: array::len($terminal) - array::len($done),
        todo: array::len($rows) - array::len($terminal),
        elements_total: math::sum($rows.subtree),
        elements_done: math::sum($done.subtree),
        by_status: (select status, count() as cnt from gen_root
            where dbnum = $dbnum group by status)
    };
};
