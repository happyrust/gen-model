--通过传递refno，查询属性值
return (select value in.refno.* from (select * from (type::thing("pe", $refno))<-pe_owner order by order_num));

