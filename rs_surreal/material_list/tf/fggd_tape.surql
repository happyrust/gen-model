
remove function fn::fggd_tape;
define function fn::fggd_tape($pes:array<record>) {
    return select
        fn::pe_to_refno(id) as id, // 参考号
        fn::fggd_bran_code(id)[0] as bran_seg_code, // 管段编号
        string::concat(fn::shape_name(id), "变截面管") as desc, // 描述
        fn::hvac_sub_code(id) as sub_code, // 子项号
        string::concat(array::at(string::split(owner.name,'-'),4),'P') as pressure_level, // 压力等级
        refno.DESP[5]?:0.0 as l, // 长
        string::concat(<string>(refno.DESP[1]?:0.0),',',<string>(refno.DESP[3]?:0.0)) as w, // 宽
        if fn::hvac_shape($this.id) == 'RECT' { string::concat(<string>(refno.DESP[2]?:0.0),',',<string>(refno.DESP[4]?:0.0)) } else { '' } as h, // 高
        (->inst_relate.world_trans.d.translation[0])[0]?:0.0 as x, // 坐标 x
        (->inst_relate.world_trans.d.translation[1])[0]?:0.0 as y, // 坐标 y
        (->inst_relate.world_trans.d.translation[2])[0]?:0.0 as z, // 坐标 z
        <string>(refno.DESP[32]?:0.0) as thickness, // 厚度
        <string>math::fixed((refno.DESP[80]?:0.0 / 1000000),2) as area, // 面积
        <string>math::fixed((refno.DESP[84]?:0.0 / 1000000),2) as weight,  // 总重
        '' as stif_sctn, // 风管加强 型钢
        '0' as stif_len, // 风管加强 长度
        '0' as stif_wei, //// 风管加强 重量
        array::first(fn::strt_flan_size($this.id)).size?:'' as flan_steel, // 法兰 型钢
        fn::tape_flan_length($this.id) as flan_length,//法兰 长度
        array::first(fn::tape_flan_weight($this.id)).weight as flan_weight, //法兰 总重
        //fn::tape_gask_size($this.id) as gask_size, //垫片类型
        //fn::tape_gask_length($this.id) as shim_width, // 垫片 长度
        // fn::tape_bolt_count($this.id) as bolt_count,  // 螺栓数量
        '' as other_material_count, // 其它材料数量
        '' as screw, // 螺杆
        // fn::tape_bolt_count($this.id) as nut, // 螺母
        // string::concat(<string>(<float>string::split(fn::tape_bolt_count($this.id),',')[0] * 2),',',<string>(<float>string::split(fn::tape_bolt_count($this.id),',')[1] * 2)) as nut_count, // 螺母数量
        fn::room_code(id)[0]?:'' as room_num , // 房间号
        array::at(string::split(owner.name,'-'),1) as system_name, // 系统号
        array::join([array::first(fn::hvac_material($this.id)).r,fn::get_ispec_material($this.id)],';') as material, // 材质
        '' as hvac_thick // 风管壁厚
    from $pes;
};

-- return fn::fggd_tape([pe:24381_57644]);

