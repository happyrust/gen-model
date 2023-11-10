--通过传递refno，查询属性值
return (select * from only (type::thing("pe", $refno)));
