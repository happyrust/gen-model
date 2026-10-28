//定义一些工艺专业的公共函数
//
// 管件的目录 P 点在 `pe_ptset` 表（ptset 分表计划，`docs/plans/2026-10-26-ptset-split-table-plan.md`；行由 gen-model
// `fast_model::ptset_rows` 在模型行的同一笔发布事务里写出，plant-ui #2）：
//   pe_ptset:⟨word0_word1⟩ = { pe, root, dbnum, noun, ptset: { "<P 号>": { pt: [x,y,z], dir: [x,y,z], bore, noun } },
//                            count, unresolved: [{ number, reason }], sesno, generator, reason, written_at }
// 坐标是管件局部系（mm）；下面只做点距与夹角，与参考系无关。没有行 = 这根还没按新版发布过；有行但 `ptset: {}` = 这件真没有 P 点（`reason` 说明）。
// 旧管线的 inst_relate.out.ptset / out.param.PrimCTorus 已随烘焙网格退役，这里不再读它们；`inst_relate.out.ptset.d.pt AS pts` 那几处投影恒为 NONE，`pe_ptset` 是唯一权威。
//
// 存储架构版本（aios_core `rs_surreal/model_storage.rs`）：整库一条记录 `storage_version:current`，
// 三格 `version` / `shape` / `rebuild`。它回答「这份库的模型面是哪一版代码建的」，不针对某张表、也不针对某个 dbnum。
// 没有这条记录、或 `version` 不等于 `MODEL_STORAGE_VERSION` 时，`fn::pe_ptset_of` 直接 THROW
// `MODEL_STORAGE_VERSION_MISMATCH …`，不再静默回 NONE —— 否则老库上的长度函数会静默回 0.0，被误当成「这根没有点」。
// 这与 `generator`（e3d-model 的 PTSET_EVAL_VERSION，说「点算得对不对」）是两条正交的轴。

// 表与索引（W3）：`IF NOT EXISTS` 重复灌不报错；清行走 `DELETE (SELECT VALUE id FROM pe_ptset WHERE root = …)`，靠 `pe_ptset_root`。
define table if not exists pe_ptset schemaless;
define index if not exists pe_ptset_root on table pe_ptset fields root;

remove function fn::pe_ptset_of;
//一个管件的 P 点集（按 P 号字符串取：$ptset['1'].pt）。
//
// 存储架构版本对不上时 THROW，返回的错误文本含稳定的码 `MODEL_STORAGE_VERSION_MISMATCH`；
// rs-server 的 `error::Error::from_anyhow` 按这个码把它映射成 409。没有行时仍是 NONE。
define function fn::pe_ptset_of($pe:record) {
    // 与 aios_core::model_storage::MODEL_STORAGE_VERSION 同值。
    // SurrealQL 里没有跨语言常量，这里是唯一一处手抄；改版本号时两处一起改，
    // gen-model 的 `embedded_surql` 测试会钉住这个字面量（见 `fast_model::ptset_rows` 的源码钉）。
    let $storage_version_expected = 1;
    let $dbnum = $pe.dbnum;
    let $storage_version = (select value version from only storage_version:current);
    // `?? 'none'` 是必须的：`<string> none` 在 SurrealDB 里抛 ConvertTo 错误，会把「版本不匹配」
    // 变成一句看不出原因的转换失败（`$pe.dbnum` 缺失时正好走到这里）。
    if $storage_version != $storage_version_expected {
        throw 'MODEL_STORAGE_VERSION_MISMATCH dbnum=' + <string> ($dbnum ?? 'none') + ' stored=' + <string> ($storage_version ?? 'none') + ' expected=' + <string> $storage_version_expected + ' source=version: 该库需 POST /api/v1/dbnums/' + <string> ($dbnum ?? 'none') + '/model/rebuild';
    };
    let $row = (select value ptset from only type::thing('pe_ptset', record::id($pe)));
    return $row;
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
        let $a = $ptset['1'].pt ?? $ptset[<string> $pe.refno.ARRI].pt;
        let $b = $ptset['2'].pt ?? $ptset[<string> $pe.refno.LEAV].pt;
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
