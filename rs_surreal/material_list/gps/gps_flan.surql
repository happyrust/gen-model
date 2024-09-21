// 给排水 大宗材料
// 法兰
remove function fn::gps_flan;
define function fn::gps_flan($refnos: array<record>){
    return select 
        fn::pe_to_refno(refno) as id,
        fn::code(id) as code, // 编码
        refno.TYPE as type ,// 部件
        math::fixed(refno.SPRE.refno.CATR.refno.PARA[6]?:0.0,3) as radius, // 外径
        math::fixed(refno.SPRE.refno.CATR.refno.PARA[4]?:0.0,3) as thick // 厚度
    from $refnos;
};

-- let $refno = [pe:24383_67888];
-- let $flan = fn::dzcl_flan($refno);
-- return $flan;
