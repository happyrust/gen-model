--通过传递refno，查询属性值
select value in.* from (select * from (type::thing("pe", $refno))<-pe_owner order by order_num) where in.deleted=false
