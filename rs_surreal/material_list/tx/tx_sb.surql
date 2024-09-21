-- 通讯设备

remove function fn::tx_sb;
define function fn::tx_sb($pes: array<record>) {
    return select 
        fn::pe_to_refno(refno) as id,
        fn::default_name(owner) as equi_name, // 设备位号
        string::slice(refno.CATR.refno.PRTREF.refno.DESC?:'',1) as ptre_desc, // 设备名称
        string::slice(string::split((->pe_owner.out->pe_owner.out.name)[0]?:'', '-')[0],1,3) as belong_factory, // 所属厂房编号
        fn::room_code(id)[0]?:'' as room_code, // 房间号
        fn::get_world_pos(id)[0][0]?:0.0 as x, // 坐标 x
        fn::get_world_pos(id)[0][1]?:0.0 as y, // 坐标 y
        fn::get_world_pos(id)[0][2]?:0.0 as z, // 坐标 z
        string::slice(refno.CATR.refno.PRTREF.refno.NAME?:' ',1) as ptre_name // 设备型号
    from $pes where !string::contains(fn::find_ancestor_type(id,'ZONE')[0].name?:'','FAD');
};

-- let $pes = [pe:24384_24895];
-- let $sb = fn::tx_sb($pes);
-- return $sb;