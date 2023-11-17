--通过传递refno，查询属性值
return (select * from (type::thing("pe", $refno)))[0];
