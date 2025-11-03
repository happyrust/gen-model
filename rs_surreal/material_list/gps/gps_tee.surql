// 给排水 大宗材料
// tee

remove function fn::gps_tee;
DEFINE FUNCTION fn::gps_tee($refnos: array<record>) {
    return select 
        fn::pe_to_refno(id) as id,
        fn::code(id) as code, // 编码
        refno.TYPE as type, // 部件
        array::join([math::fixed(refno.SPRE.refno.CATR.refno.PARA[6]?:0, 3),
            math::fixed(refno.SPRE.refno.CATR.refno.PARA[7]?:0, 3)],';') as radius // 外径
    from $refnos;
};

-- let $refno = [pe:24383_74117];
-- return fn::gps_tee($refno);

// select refno,
// string::split(string::split(if refno.SPRE.name == NONE { "//:" } else { refno.SPRE.name },'/')[2],':')[0] as code, // 编码
// refno.TYPE as type, // 部件
// string::replace(array::join([<string>math::fixed(if refno.SPRE.refno.CATR.refno.PARA[6] != NONE { refno.SPRE.refno.CATR.refno.PARA[6] } else { 0 },3),<string>math::fixed(if refno.SPRE.refno.CATR.refno.PARA[7] != NONE { refno.SPRE.refno.CATR.refno.PARA[7] } else { 0 },3)],';'),'f','') as radius // 外径
// from $refno;