// bend
// let $refno = [pe:24383_84092];

remove function fn::gy_bend;
define function fn::gy_bend($refnos: array<record>) {
    return select
        fn::pe_to_refno(id) as id,
        fn::code(id) as code, // 编码
        refno.TYPE as noun, // 部件
        math::fixed((refno.ANGL?:0.0 / 360) * 2 *3.1415 * refno.SPRE.refno.CATR.refno.PARA[1]?:0.0, 2) as length // 长度
    from $refnos where noun == 'BEND';
};

-- let $refno = [pe:24383_84092];
-- let $bend = fn::gy_bend($refno);
-- return $bend;
