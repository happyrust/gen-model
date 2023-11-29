--通过传递refno，查询children
select value id from type::thing("pe", $refno);
select value in from (select * from type::thing("pe", $refno)<-pe_owner order by order_num) where in.deleted=false;
