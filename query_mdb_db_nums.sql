-- 查询MDB数据库编号
-- 从MDB表中获取指定数据库类型对应的DBNO列表
-- 然后从WORL表中按顺序获取对应的dbnum

-- 参数说明:
-- $mdb: MDB数据库名称
-- $db_type: 数据库类型 (1=DESI, 2=CATA, 3=PROP, 4=ISOD, 5=PADD, 6=DICT, 7=ENGI, 14=SCHE)

-- SurrealQL查询语句
let $dbnos = select value (select value DBNO from CURD.refno where STYP=$db_type) from only MDB where NAME=$mdb limit 1;
select value dbnum from (select REFNO.dbnum as dbnum, array::find_index($dbnos, REFNO.dbnum) as o
    from WORL where REFNO.dbnum in $dbnos order by o);

-- 分步骤解释:
-- 1. 首先根据MDB名称和数据库类型获取对应的DBNO列表
-- 2. 然后查询WORL表中匹配这些DBNO的记录
-- 3. 使用array::find_index确定记录顺序，并按顺序返回dbnum列表

-- 示例用法:
-- bind(("mdb", "your_mdb_name"))
-- bind(("db_type", 1)) -- 1表示DESI类型
