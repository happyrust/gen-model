// let $refno = [pe:24383_67446,pe:24383_67451]

remove function fn::gy_part;
DEFINE FUNCTION fn::gy_part($refnos: array<record>) {
    let $nouns = [
                "ELBO",
                "TEE",
                "FLAN",
                "GASK",
                "OLET",
                "REDU",
                "CAP",
                "COUP",
            ];
    return select 
        fn::pe_to_refno(id) as id, //参考号
        fn::code(id) as code, // 编码
        refno.TYPE as noun // 部件
        //refno.DESP[1] as count // 数量
    from $refnos where refno.TYPE in $nouns;
};
-- let $data = fn::gy_part($refno);
-- insert into gy_material $data;
-- let $refno = [pe:24383_68329];
-- return fn::gy_part($refno);

