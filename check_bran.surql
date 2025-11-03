-- 查询 21491_18946 的基本信息
select * from pe:21491_18946;

-- 查询它的子元素
select * from pe:21491_18946.children;

-- 查询它的 tubi_relate
select * from pe:21491_18946->tubi_relate;

-- 查询它的属性
select noun, NAME, HPOS, HDIR, TPOS, TDIR, HREF, TREF, HSTU, LSTU from pe:21491_18946;

