--通过传递refno，查询属性值
return (select value refno.* from (type::thing("pe", $refno)))[0];
