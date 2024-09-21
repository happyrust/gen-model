// 给排水 大宗材料
// elbo

remove function fn::gps_elbo;
define function fn::gps_elbo($refnos: array<record>){
    return select 
        fn::pe_to_refno(id) as id,
        fn::code(id) as code, // 编码
        refno.TYPE as type ,// 部件
        math::fixed(refno.SPRE.refno.CATR.refno.PARA[3]?:0,3) as radius // 外径
    from $refnos;
}

-- let $refno = [pe:24383_74103];
-- let $elbo = fn::dzcl_elbo($refno);
-- return $elbo;