// 给排水 大宗材料
// tubi

remove function fn::gps_tubi;
define function fn::gps_tubi($refnos: array<record>){
    return select
      fn::pe_to_refno(leave) as id,
      string::split(
          array::at(string::split((leave.refno.HSTU ?: (leave.refno.LSTU)).name?:'', '/'), 2) ?: '',
           ':'
        )[0]?:'' as code,
        'TUBI' as type,
      math::fixed(if leave.refno.LSTU != NONE { 
              leave.refno.LSTU.refno.CATR.refno.PARA[1] ?: 0  
          } else if leave.refno.HSTU != NONE { 
              leave.refno.HSTU.refno.CATR.refno.PARA[1] ?: 0
          } else { 0 }, 3 ) as radius, // 外径
      world_trans.d.scale[2]?:0 as length
    from array::flatten($refnos->tubi_relate);
};

-- let $refno = [pe:24383_66627];
-- return fn::dzcl_tubi($refno);


//   select value (select leave as id,
// (select value ( if leave.refno.LSTU.refno.NAME != NONE { string::split(array::at(string::split(leave.refno.LSTU.name, '/'), 2), ':')[0] } else if leave.refno.HSTU.refno.NAME != NONE {
//   string::split(array::at(string::split(leave.refno.HSTU.name, '/'), 2), ':')[0]
// } else { '' }  ) from $self)[0]  as code,
// 'TUBI' as type,
// math::fixed(if leave.refno.LSTU.refno.NAME != NONE { leave.refno.LSTU.refno.CATR.refno.PARA[1] } else if leave.refno.HSTU.refno.NAME != NONE { leave.refno.HSTU.refno.CATR.refno.PARA[1] } else { 0 },3 ) as radius, // 外径
// world_trans.d.scale[2] as count from ->tubi_relate) from {};