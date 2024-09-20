-- 阀门列表
remove function fn::nt_valv;
define function fn::nt_valv($refnos: array<record>) {
    return select 
        fn::pe_to_refno(refno) as id,
        fn::default_name(id) as name, // 阀门位号
        fn::room_code(id)[0]?:'' as room_code, // 所在房间号
        (->pe_owner.out->pe_owner.in.refno.NAME)[0]?:'' as bran_name, // 阀门归属
        [ refno.DESP[1]?:0, refno.DESP[2]?:0, refno.DESP[5]?:0 ] as valv_size, // 阀门尺寸
        fn::get_valv_material(id) as material, // 阀门材质
        string::slice(name?:'', -3) as valv_use // 阀门功能
    from $refnos;
};

-- let $refno = [pe:24383_84088];
-- let $valv_list = fn::nt_valv($refno);
-- return $valv_list;



