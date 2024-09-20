
// 材质
remove function fn::hvac_material;
DEFINE FUNCTION fn::hvac_material($pe:record) {
    let $mat_code_map = ['','SS', 'GS'	,'CS','ACS','ASS',	'AS', 'FP'];
    return
    select if array::at(string::split(owner.name,'-'),3) != '6' {
        array::at($mat_code_map,<int>array::at(string::split(owner.name,'-'),3))
    } else if fn::pre_or_next_b_type($this.id,'DAMP') { 'CS' } else {
        'FP'
    } as r from $pe
};

remove function fn::fggd_strt;
define function fn::fggd_strt($pes:array<record>) {
    return select
        fn::pe_to_refno(id) as id, // 参考号
        fn::fggd_bran_code(id)[0] as bran_seg_code, // 管段编号
        string::concat(fn::shape_name(id), "直管") as desc,
        fn::hvac_sub_code(id) as sub_code, // 子项号
        // array::first(fn::hvac_material($this.id)).r as mat,// 材质
        string::concat(array::at(string::split(owner.name,'-'),4),'P') as pressure_level, // 压力等级
        refno.DESP[5] as l, // 长
        if fn::hvac_shape($this.id) == 'RECT' { refno.DESP[1]?:0.0 } else if fn::hvac_shape($this.id) == 'CIRC' { string::concat('φ',string::slice(<string>refno.DESP[1],0,string::len(<string>refno.DESP[1])-1)) } else { refno.DESP[1]?:0.0 } as w, // 宽
        if fn::hvac_shape($this.id) == 'RECT' { refno.DESP[3]?:0.0 } else if fn::hvac_shape($this.id) == 'CIRC' { '' } else { refno.DESP[3]?:0.0 } as h, // 高
        (->inst_relate.world_trans.d.translation[0])[0]?:0.0 as x, // 坐标 x
        (->inst_relate.world_trans.d.translation[1])[0]?:0.0 as y, // 坐标 y
        (->inst_relate.world_trans.d.translation[2])[0]?:0.0 as z, // 坐标 z
        if array::first(fn::pre_or_next_b_type(id,'DAMP')) { 3 } else { refno.DESP[32]?:0.0 } as thickness, // 风管壁厚
        if array::first(fn::pre_or_next_b_type(id,'DAMP')) { math::fixed(refno.DESP[80] / 1000000,2) } else { math::fixed(refno.DESP[1] * refno.DESP[2] / 1000000 * 150,2) } as area, // 风管面积
        if array::first(fn::pre_or_next_b_type(id,'DAMP')) { math::fixed((refno.DESP[1] * refno.DESP[2] * 150 * (if array::first(fn::pre_or_next_b_type(refno,'DAMP')) { 3 } else { refno.DESP[32] }) * 7.85) / 1000000,2) } else { math::fixed(refno.DESP[84]?:0.0,2) } as weight,  // 风管总重
        array::first(fn::hvac_size($this.id,fn::hvac_shape($this.id),array::at(string::split(owner.name,'-'),4)?:'')).size as stif_sctn, // 风管加强 型钢
        math::fixed(array::first(fn::hvac_length($this.id,fn::hvac_shape($this.id),array::at(string::split(owner.name,'-'),4)?:'',refno.DESP[5]?:0.0,refno.DESP[1]?:0.0,refno.DESP[2]?:0.0)) / 1000,2) as stif_len, // 风管加强 长度
        math::fixed(array::first(fn::hvac_weight($this.id,fn::hvac_shape($this.id),array::at(string::split(owner.name,'-'),4)?:'',refno.DESP[5]?:0.0,refno.DESP[1]?:0.0,refno.DESP[2]?:0.0)) / 1000,2) as stif_wei, //// 风管加强 重量
        array::first(fn::strt_flan_size($this.id)).size as flan_steel, // 法兰 型钢
        math::fixed(array::first(fn::strt_flan_length($this.id)).length,2) as flan_length,//法兰 长度
        math::fixed(array::first(fn::strt_flan_weight($this.id)).weight,2) as flan_weight, //法兰 总重
        array::first(fn::strt_gask_size($this.id)).size as gask_size, //垫片类型
        math::fixed(array::first(fn::strt_gask_length($this.id,refno.DESP[20])).size,2) as shim_width, // 垫片 长度
        math::fixed(array::first(fn::strt_bolt_count($this.id,refno.DESP[20])).count,2) as bolt_count,  // 螺栓数量
        '' as other_material_count, // 其它材料数量
        '' as screw, // 螺杆
        math::fixed(array::first(fn::strt_bolt_count($this.id,refno.DESP[20]?:0.0)).count,2) as nut_count, // 螺母数量
        math::fixed(array::first(fn::strt_bolt_count($this.id,refno.DESP[20]?:0.0)).count * 2,2) as nut_count_b, // 螺母数量
        fn::room_code(id)[0]?:'' as room_num , // 房间号
        array::at(string::split(owner.name,'-'),1) as system_name, // 系统号
        array::join([array::first(fn::hvac_material($this.id)).r,fn::get_ispec_material($this.id)],';') as material // 材质
    from $pes;
};

// return fn::fggd_strt([pe:24381_100820])