#仪控专业
#定义方法
remove function fn::yk_dzcl;
define function fn::yk_dzcl($refnos: array<record>) {
    return select 
    fn::pe_to_refno(id) as id,
    string::split(string::split(refno.SPRE.name?:'/', '/')[2]?:':',':')[0] as code ,
    refno.TYPE as type // 品名
    from $refnos;
}

-- let $refnos = [pe:24383_67652];
-- let $dzcl = fn::dzcl($refnos);
-- return $dzcl;


-- select id,
-- if refno.SPRE.name != NONE {{ string::split(string::split(refno.SPRE.name,'/')[2],':')[0] }} else {{ ' ' }} as code ,// 编码
-- refno.TYPE as type // 品名
-- from [pe:24383_67652];
