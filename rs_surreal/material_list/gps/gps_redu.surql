// 给排水 大宗材料
// redu

remove function fn::gps_redu;
define function fn::gps_redu($refnos: array<record>){
    return select 
        fn::pe_to_refno(refno) as id,
        fn::code(id) as code, // 编码
        refno.TYPE as type ,// 部件
        array::join([math::fixed(refno.SPRE.refno.CATR.refno.PARA[5]?:0.0,3),
        math::fixed(refno.SPRE.refno.CATR.refno.PARA[6]?:0.0,3)],';') as radius, // 外径 
        math::fixed(refno.SPRE.refno.CATR.refno.PARA[3]?:0.0,3) as length // 长度
    from $refnos;
};

-- let $refno = [pe:24383_67889];
-- let $redu = fn::dzcl_redu($refno);
-- return $redu;
