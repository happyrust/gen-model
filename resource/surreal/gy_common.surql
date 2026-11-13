//定义一些工艺专业的公共函数
//
// 管件的目录 P 点在 `pe_ptset` 表（ptset 分表计划，`docs/plans/2026-10-26-ptset-split-table-plan.md`；行由 gen-model
// `fast_model::ptset_rows` 在模型行的同一笔发布事务里写出，plant-ui #2）：
//   pe_ptset:⟨word0_word1⟩ = { pe, root, dbnum, noun, ptset: { "<P 号>": { pt: [x,y,z], dir: [x,y,z], bore, noun } },
//                            count, unresolved: [{ number, reason }], sesno, generator, reason, written_at }
// 坐标是管件局部系（mm）；下面只做点距与夹角，与参考系无关。没有行 = 这根还没按新版发布过；有行但 `ptset: {}` = 这件真没有 P 点（`reason` 说明）。
// 旧管线的 inst_relate.out.ptset / out.param.PrimCTorus 已随烘焙网格退役，这里不再读它们；`inst_relate.out.ptset.d.pt AS pts` 那几处投影恒为 NONE，`pe_ptset` 是唯一权威。
//
// 数据库架构版本（aios_core `rs_surreal/model_storage.rs`）：整库一条记录 `storage_version:current`。
// 版本只在进程启动时全局核对一次；本文件里的读点集函数**不做任何版本判断**，没有行就是 NONE。
// 这与 `generator`（e3d-model 的 PTSET_EVAL_VERSION，说「点算得对不对」）是两条正交的轴。

// 表与索引（W3）：`IF NOT EXISTS` 重复灌不报错；清行走 `DELETE (SELECT VALUE id FROM pe_ptset WHERE root = …)`，靠 `pe_ptset_root`。
define table if not exists pe_ptset schemaless;
define index if not exists pe_ptset_root on table pe_ptset fields root;

remove function fn::pe_ptset_of;
//一个管件的 P 点集（按 P 号字符串取：$ptset['1'].pt）。
//
// 读点集不做版本判断：版本只在进程启动时全局核对一次；没有行时返回 NONE。
define function fn::pe_ptset_of($pe:record) {
    return (select value ptset from only type::thing('pe_ptset', record::id($pe)));
};

remove function fn::bran_tubi_len;
//首先查询所有 tubi 的长度 
define function fn::bran_tubi_len($pe:record) {
    return if $pe.noun != 'BRAN' {
        return 0;
    } else {
        return math::sum(select value world_trans.d.scale[2] from $pe->tubi_relate);
    }
};

//计算弯头 / 弯管的中心线弧长：两端 P 点的弦长 + 弯转角（两端方向都朝外，转角 = π - 夹角），直通退化为弦长
remove function fn::elbo_len;
define function fn::elbo_len($pe:record) {
    return if $pe.noun != 'ELBO' && $pe.noun != 'BEND' {
        return 0;
    } else if $pe.refno.ARRI == none || $pe.refno.LEAV == none {
        return 0.0;
    } else {
        let $ptset = fn::pe_ptset_of($pe);
        let $arri = $ptset[<string> $pe.refno.ARRI];
        let $leav = $ptset[<string> $pe.refno.LEAV];
        return if $arri.pt == none || $leav.pt == none {
            return 0.0;
        } else {
            let $chord = vector::distance::euclidean($arri.pt, $leav.pt);
            let $theta = if $arri.dir == none || $leav.dir == none {
                0.0
            } else {
                math::pi - vector::angle($arri.dir, $leav.dir)
            };
            return if $theta < 0.0001 || $chord == 0.0 {
                return $chord;
            } else {
                return $chord / (2.0 * math::sin($theta / 2.0)) * $theta;
            };
        };
    }
};

//三通：主管长 + 支管长。老函数求的是主管圆柱与支管圆柱的长度和，与流向无关，所以主管两端按目录惯例取 P1 / P2、
//支管口 P3（支管长 = P3 到主管轴线的垂距）；不能按 ARRI / LEAV 取：流向 1 → 3 的三通那样算出的是一条斜弦。P1 / P2 缺席才退回 ARRI / LEAV。
remove function fn::tee_len;
define function fn::tee_len($pe:record) {
    return if $pe.noun != 'TEE' {
        0
    } else {
        let $ptset = fn::pe_ptset_of($pe);
        let $arri_pt = if $pe.refno.ARRI == none { none } else { $ptset[<string> $pe.refno.ARRI].pt };
        let $leav_pt = if $pe.refno.LEAV == none { none } else { $ptset[<string> $pe.refno.LEAV].pt };
        let $a = $ptset['1'].pt ?? $arri_pt;
        let $b = $ptset['2'].pt ?? $leav_pt;
        let $p3 = $ptset['3'].pt;
        return if $a == none || $b == none {
            return 0.0;
        } else {
            let $run = vector::distance::euclidean($a, $b);
            let $branch = if $p3 == none || $run == 0.0 {
                0.0
            } else {
                vector::magnitude(vector::cross(vector::subtract($p3, $a), vector::subtract($b, $a))) / $run
            };
            return $run + $branch;
        };
    }
};

remove function fn::bran_len;
define function fn::bran_len($pe:record) {
   //首先查询所有 tubi 的长度 
   let $tubi_len = fn::bran_tubi_len($pe);
   let $len = math::sum(select value fn::bran_comp_len(in) from $pe<-pe_owner);
   return $tubi_len + $len;
};

//计算管道元件的长度：直通件取 P(LEAV) 到 P(ARRI) 的距离，弯头 / 三通分别走上面两个函数
remove function fn::bran_comp_len;
define function fn::bran_comp_len($pe:record) {
   return if $pe.noun == 'ELBO' || $pe.noun == 'BEND' {
       return fn::elbo_len($pe);
   } else if $pe.noun == 'TEE' {
       return fn::tee_len($pe);
   } else if $pe.refno.LEAV == none || $pe.refno.ARRI == none {
       return 0.0;
   } else {
       let $ptset = fn::pe_ptset_of($pe);
       let $l = <string> $pe.refno.LEAV;
       let $a = <string> $pe.refno.ARRI;
       let $start_pt = $ptset[$l].pt;
       let $end_pt = $ptset[$a].pt;
       return if $start_pt == none || $end_pt == none {
           0.0;
       } else {
           vector::distance::euclidean($start_pt, $end_pt);
       }
   };
};
