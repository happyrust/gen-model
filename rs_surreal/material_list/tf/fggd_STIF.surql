//STIFF
remove function fn::fggd_STIF;
DEFINE FUNCTION fn::fggd_STIF($pes: array<record>) {
    return select
    fn::pe_to_refno(id) as id, // 参考号
    string::concat(fn::shape_name(id), "加强筋") as desc, // 描述
    fn::fggd_bran_code(id)[0] as bran_seg_code, // 管段编号
    fn::hvac_sub_code(id) as sub_code, // 子项号
    array::join([array::first(fn::hvac_material($this.id)).r,fn::get_ispec_material($this.id)],';') as material, // 材质
    fn::hvac_pressure(id) as pressure_level, // 压力等级
    fn::hvac_len(id) as l, // 风管长度
    fn::hvac_width_format(id) as w, // 风管宽度
    fn::hvac_height_format(id) as h, // 风管高度
    fn::hvac_thks(id, "DAMP") as thickness, //  风管壁厚
    fn::hvac_duct_areas(id, "DAMP") as area, // 风管面积
    fn::hvac_duct_weight_format(id, "DAMP") as weight, // 风管总重
    fn::cal_stif_sctn_str(id) as stif_sctn, // 风管加强型钢
    fn::cal_stif_len_str(id) as stif_len, // 风管加强长度
    fn::cal_stif_wei_str(id) as stif_wei, // 风管加强重量
    '-' as flan_steel, // 法兰型钢
    '-' as flan_length, // 法兰长度
    '-' as flan_weight, // 法兰重量

    '-' as washer_type,
    '-' as washer_len,
    '-' as bolt_qty,
    '-' as other_type,
    '-' as other_qty,
    '-' as stud,

    fn::hvac_bolt_qty(id) as nut_count,  //螺母数量
    fn::hvac_bolt_qty(id) * 2 as nut_count_b, // 螺母数量_2

    fn::room_code(id)?:'' as room_num,
    fn::hvac_system(id) as system_name


    from $pes;
}