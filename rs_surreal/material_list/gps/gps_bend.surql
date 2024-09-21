// 给排水 大宗材料
// bend

-- 弯头
remove function fn::gps_bend;
define function fn::gps_bend($refnos: array<record>){
    return select 
        fn::pe_to_refno(refno) as id,
        fn::code(id) as code, // 编码
        refno.TYPE as type ,// 部件
        math::fixed(refno.SPRE.refno.CATR.refno.PARA[3]?:0.0,3) as radius, // 外径
        math::fixed((refno.ANGL?:0.0 / 360) * 2 *3.1415 * refno.SPRE.refno.CATR.refno.PARA[1]?:0.0,2) as length // 数量
    from $refnos;
};

// let $refno = [pe:24384_2967];
// let $bend = fn::gps_bend($refno);
// return $bend;


