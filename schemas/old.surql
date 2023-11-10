--通过传递refno，查询属性值
let $type_name = (select value noun from (type::thing("pe", $refno)));
IF array::len($type_name) == 1 {
    return (select * from (type::thing($type_name[0], $refno)))[0];
}