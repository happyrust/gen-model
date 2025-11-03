-- 设备清单
remove function fn::eq_dz;
DEFINE FUNCTION fn::eq_dz($refnos: array<record>) {
    return select 
        fn::pe_to_refno(refno) as id,
        fn::default_name(id) as name, // 设备位号   
        fn::room_code(id)[0]?:'' as room_code, // 所在房间
        fn::find_group_box_size_and_high_z(id)[0]?:0.0 as pos, // 安装标高
        fn::find_group_box_size_and_high_z(id)[1]?:0.0 as length // 轨道长度
    from $refnos where name != NONE && ( string::contains(name,'PR') || string::contains(name,'PD') );   
};

-- let $refno = [pe:24381_57828,pe:24383_76416];
-- let $equip = fn::eq_dz($refno);
-- return $equip;

