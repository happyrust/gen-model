// 底标高
REMOVE FUNCTION fn::room_height;
DEFINE FUNCTION fn::room_height($pe:record) {
    let $noun = $pe.refno.TYPE;
    // 面板几何读法（2026-10-29 修）：room_relate.in 是面板 pe，几何在**普通表**
    // inst_relate:⟨ref0_ref1⟩ 上（不是从 pe 出去的边），所以旧写法
    // `<-room_relate.in.id->inst_relate.aabb.d.mins[2]` 恒为 NONE（真库实测 [null]）。
    // 现在先取面板（跨多房按 ADR-010 §5 最强边），再按键读 aabb_d。
    // array::filter 滤掉 NONE：消费方（core query_own_room_panel_elevations）按
    // Vec<f32> 解码，混入 NONE 会让整条解码失败并静默落 0.0。
    return
    // EQUI可能有两层
    if $noun == 'EQUI' || $noun == 'STRU' || $noun == 'REST' {
        let $deep = array::flatten([
            $pe<-pe_owner.in.id,
            array::flatten($pe<-pe_owner.in<-pe_owner.in.id)
        ]);
        let $panels = array::filter((SELECT VALUE fn::room_relate_of(id).in FROM $deep), |$v| $v != NONE);
        return array::distinct(array::filter(
            array::map($panels, |$p| type::thing('inst_relate', record::id($p)).aabb_d.mins[2]),
            |$v| $v != NONE
        ))
    } else if $noun == 'BRAN' {
        let $kids = array::flatten($pe<-pe_owner.in.id);
        let $panels = array::filter((SELECT VALUE fn::room_relate_of(id).in FROM $kids), |$v| $v != NONE);
        return array::distinct(array::filter(
            array::map($panels, |$p| type::thing('inst_relate', record::id($p)).aabb_d.mins[2]),
            |$v| $v != NONE
        ))
    } else  {
        let $panels = array::filter([fn::room_relate_of($pe).in], |$v| $v != NONE);
        return array::distinct(array::filter(
            array::map($panels, |$p| type::thing('inst_relate', record::id($p)).aabb_d.mins[2]),
            |$v| $v != NONE
        ))
    }
};

// 顶标高
REMOVE FUNCTION fn::room_top_height;
DEFINE FUNCTION fn::room_top_height($pe:record) {
    let $noun = $pe.refno.TYPE;
    return
    // EQUI可能有两层
    if $noun == 'EQUI' || $noun == 'STRU' || $noun == 'REST' {
        let $deep = array::flatten([
            $pe<-pe_owner.in.id,
            array::flatten($pe<-pe_owner.in<-pe_owner.in.id)
        ]);
        let $panels = array::filter((SELECT VALUE fn::room_relate_of(id).in FROM $deep), |$v| $v != NONE);
        return array::distinct(array::filter(
            array::map($panels, |$p| type::thing('inst_relate', record::id($p)).aabb_d.maxs[2]),
            |$v| $v != NONE
        ))
    } else if $noun == 'BRAN' {
        let $kids = array::flatten($pe<-pe_owner.in.id);
        let $panels = array::filter((SELECT VALUE fn::room_relate_of(id).in FROM $kids), |$v| $v != NONE);
        return array::distinct(array::filter(
            array::map($panels, |$p| type::thing('inst_relate', record::id($p)).aabb_d.maxs[2]),
            |$v| $v != NONE
        ))
    } else  {
        let $panels = array::filter([fn::room_relate_of($pe).in], |$v| $v != NONE);
        return array::distinct(array::filter(
            array::map($panels, |$p| type::thing('inst_relate', record::id($p)).aabb_d.maxs[2]),
            |$v| $v != NONE
        ))
    }
};


// return fn::room_height(pe:24384_24804)[0];
