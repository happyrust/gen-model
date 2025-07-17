remove function fn::fggd_bend;
define function fn::fggd_bend($pes:array<record>) {
    return select
        fn::pe_to_refno(id) as id, // 参考号
        fn::fggd_bran_code(id)[0] as bran_seg_code, // 管段编号
        if fn::hvac_shape($this.id) == 'RECT' { '方形弯管' } else if fn::hvac_shape($this.id) == 'CIRC' { '圆形弯管' } else { '' } as desc,
        fn::hvac_sub_code(id) as sub_code, // 子项号
        array::first(fn::hvac_material($this.id)).r as mat,// 材质
        string::concat(array::at(string::split(owner.name,'-'),4),'P') as pressure_level, // 压力等级
        refno.DESP[5]?:0.0 as l, // 长
        string::concat(<string>refno.DESP[1]?:0.0,',',<string>refno.DESP[3]?:0.0) as w, // 宽
        if fn::hvac_shape($this.id) == 'RECT' { string::concat(<string>refno.DESP[2]?:0.0,',',<string>refno.DESP[4]?:0.0) } else { '' } as h, // 高
        (->inst_relate.world_trans.d.translation[0])[0]?:0.0 as x, // 坐标 x
        (->inst_relate.world_trans.d.translation[1])[0]?:0.0 as y, // 坐标 y
        (->inst_relate.world_trans.d.translation[2])[0]?:0.0 as z, // 坐标 z
        array::join([<string>(refno.DESP[32]?:0.0),<string>array::first(fn::bend_hvac_thick($this.id)).size,fn::get_ispec_hvac_thick($this.id)],';')  as thickness, // 风管壁厚
        array::join([<string>math::fixed(refno.DESP[80]?:0.0 / 1000000,2),math::fixed(array::first(fn::bend_hvac_area($this.id)).size,2)],';') as area, // 风管面积
        array::join([<string>math::fixed(refno.DESP[84]?:0.0,3),<string>math::fixed(array::first(fn::bend_hvac_weight($this.id).size),2)],';') as weight,  // 风管总重
        '' as steel, // 风管加强 型钢
        '0' as hvac_length, // 风管加强 长度
        '0' as hvac_weight, //// 风管加强 重量
        array::first(fn::strt_flan_size($this.id)).size?:'' as flan_steel, // 法兰 型钢
        array::first(fn::flex_flan_length($this.id)).length as flan_length, //法兰 长度
        <string>math::fixed(array::first(fn::flex_flan_weight($this.id)).weight?:0.0,2) as flan_weight, ////法兰 总重
        // fn::bend_gask_size($this.id) as gask_size, //垫片类型
        // fn::flex_gask_size($this.id) as shim_width, // 垫片 长度
        array::join([<string>math::fixed(array::first(fn::bend_bolt_count($this.id)).count?:0.0,2),array::first(fn::bend_bolt_count_2($this.id)).count],';') as bolt_count,  // 螺栓数量
        array::join([' ',array::first(fn::bend_other_material_type_2($this.id)).size],';') as other_material_type, // 其它材料类型
        array::join(['0',array::first(fn::bend_other_material_count_2($this.id)).size],';') as other_material_count, // 其它材料数量
        array::join(['',array::first(fn::bend_bolt_length_2($this.id)).count],';') as screw, // 螺杆长度
        array::join([<string>math::fixed(array::first(fn::bend_bolt_count($this.id)).count?:0.0,2),array::first(fn::bend_bolt_count_2($this.id)).count],';') as nut_count, // 螺母数量
        <string>(math::fixed(array::first(fn::bend_bolt_count($this.id)).count?:0.0,2) * 2) as gask_count, // 垫片数量
        fn::room_code(id)[0]?:'' as room_num , // 房间号
        array::at(string::split(owner.name,'-'),1) as system_name, // 系统号
        array::join([array::first(fn::hvac_material($this.id)).r,fn::get_ispec_material($this.id)],';') as material // 材质
        //array::first(fn::get_ipara($this.id))
        from $pes;
};

// return fn::fggd_bend([pe:24381_57305])