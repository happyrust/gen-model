remove function fn::fggd_ofst;
define function fn::fggd_ofst($pes:array<record>) {
    return select
        fn::pe_to_refno(id) as id, // 参考号
        fn::fggd_bran_code(id)[0] as bran_seg_code, // 管段编号
        // array::first(fn::hvac_material($this.id)).r as mat,// 材质
        'OFST' as desc,
        fn::hvac_sub_code(id) as sub_code, // 子项号
        string::concat(array::at(string::split(owner.name,'-'),4),'P') as pressure_level, // 压力等级
        string::concat(<string>refno.DESP[5],'-',<string>refno.DESP[9]) as l, // 风管长度
        string::replace(if fn::hvac_shape($this.id) == 'RECT' { <string>refno.DESP[1] } else if fn::hvac_shape($this.id) == 'CIRC' { string::concat('φ',<string>refno.DESP[1]) } else { '' },'f','') as w, // 宽
        if fn::hvac_shape($this.id) == 'RECT' { string::replace(<string>refno.DESP[2],'f','') } else if fn::hvac_shape($this.id) == 'CIRC' { } else { '' } as h, // 高
        (->inst_relate.world_trans.d.translation[0])[0]?:0.0 as x, // 坐标 x
        (->inst_relate.world_trans.d.translation[1])[0]?:0.0 as y, // 坐标 y
        (->inst_relate.world_trans.d.translation[2])[0]?:0.0 as z, // 坐标 z
        string::slice(<string>refno.DESP[32],0,string::len(<string>refno.DESP[32])-1)  as thickness, // 厚度
        string::slice(<string>math::fixed(refno.DESP[80] / 1000000,2),0,string::len(<string>math::fixed(refno.DESP[80] / 1000000,2)) -1 ) as area, // 面积
        string::slice(<string>math::fixed(refno.DESP[84],3),0,string::len(<string>math::fixed(refno.DESP[84],3))-1) as weight,  // 总重
        '' as steel, // 风管加强 型钢
        '0' as hvac_length, // 风管加强 长度
        '0' as hvac_weight, //// 风管加强 重量
        array::first(fn::strt_flan_size($this.id)).size as flan_steel, // 法兰 型钢 
        math::fixed(array::first(fn::flex_flan_length($this.id)).length?:0.0,2) as flan_length, //法兰 长度
        <string>math::fixed(array::first(fn::flex_flan_weight($this.id)).weight?:0.0,2) as flan_weight, ////法兰 总重
        // fn::tape_gask_size($this.id) as gask_size, //垫片类型
        array::join([<string>(array::first(fn::flex_bolt_count($this.id,refno.DESP[20]?:0.0)).count * 2) ,<string>(array::first(fn::flex_bolt_count($this.id,refno.DESP[20]?:0.0)).count * 2) ],';') as gask_count, // 垫片数量
        // fn::flex_gask_size($this.id) as shim_width, // 垫片 长度
        <string>array::first(fn::flex_bolt_count($this.id,refno.DESP[20]?:0.0)).count as bolt_count,  // 螺栓数量
        '' as other_material_type, // 其它材料类型
        '' as other_material_count, // 其它材料数量
        '' as screw, // 螺杆
        <string>array::first(fn::flex_bolt_count($this.id,refno.DESP[20]?:0.0)).count as nut_count, // 螺母
        <string>(array::first(fn::flex_bolt_count($this.id,refno.DESP[20]?:0.0)).count * 2) as nut_count_b, // 螺母数量
        fn::room_code(id)[0]?:'' as room_num , // 房间号
        array::at(string::split(owner.name,'-'),1) as system_name, // 系统号
        array::join([array::first(fn::hvac_material($this.id)).r,fn::get_ispec_material($this.id)],';') as material, // 材质
        '' as hvac_thick // 风管壁厚
        from $pes;
};

-- return fn::fggd_ofst([pe:24381_58341])

