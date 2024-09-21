
//BRCO
remove function fn::fggd_BRCO;
DEFINE FUNCTION fn::fggd_BRCO($pes: array<record>) {
   return select
      fn::pe_to_refno(id) as id, // 参考号
      string::concat(fn::shape_name(id), "连接管") as desc,
      fn::fggd_bran_code(id)[0] as bran_seg_code, // 管段编号
      fn::hvac_sub_code(id) as sub_code, // 子项号
      fn::hvac_mats(id) as material, // 材质
      fn::hvac_pressure(id) as pressure_level, // 压力等级
      fn::hvac_len(id) as l, // 风管长度
      fn::hvac_width_format(id, 3) as w, // 风管宽度
      fn::hvac_height_format(id, 3) as h, // 风管高度
      (->inst_relate.world_trans.d.translation[0])[0]?:0.0 as x, // 坐标 x
      (->inst_relate.world_trans.d.translation[1])[0]?:0.0 as y, // 坐标 y
      (->inst_relate.world_trans.d.translation[2])[0]?:0.0 as z, // 坐标 z
      fn::hvac_thks(id, "DAMP") as thickness, // 风管壁厚
      fn::hvac_duct_areas(id, "DAMP") as area, // 风管面积
      fn::hvac_duct_weight_format(id, "DAMP") as weight, // 风管总重
      fn::cal_stif_sctn_str(id) as stif_sctn,  // 风管加强型钢
      fn::cal_stif_len_str(id) as stif_len, // 风管加强长度
      fn::cal_stif_wei_str(id) as stif_wei, // 风管加强重量
      fn::hvac_fl_name(id) as flan_steel, // 法兰型钢
      fn::hvac_fl_len(id) as flan_length, // 法兰长度
      fn::hvac_fl_wei(id) as flan_weight, // 法兰重量

       fn::common_washer_types(id) as gask_size, // 垫片类型
       fn::common_washer_len(id) as shim_width, // 垫片长度

       // fn::hvac_bolt_qtys_BRCO(id) as bolt_qty,
      '-' as other_type,
      '-' as other_qty,
      '-' as stud,

       // fn::hvac_bolt_qtys_BRCO(id) as nut_qty,  //螺母数量
       fn::hvac_washer_bolt_qtys_THRE(id) as gask_count,  //垫片数量


       fn::hvac_system(id) as system_name, // 系统
       fn::room_code(id)?:'' as room_num, // 所在房间号
       array::join([array::first(fn::hvac_material($this.id)).r,fn::get_ispec_material($this.id)],';') as material // 材质

   from $pes;
}