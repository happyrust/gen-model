REMOVE FUNCTION fn::room_code;
-- 2026-10-30 修：hh 版对 $pe 的链式访问会让参数退化成整行对象，之后
-- `record::id($pe)` 与 `$pe<-pe_owner…` 一律报
-- 「Expected a record but found { id: pe:…, noun: … }」。
-- 病灶有两个，都不在入口：
--   1) `select value fn::room_num_of($this) from <记录数组>` 里的 `$this` 是**整行对象**
--      而不是记录 id，传进 `fn::room_num_of($pe: record)` 后它内部的
--      `$pe<-room_relate` 就崩。改用 `array::map(<数组>, |$p| fn::room_num_of(type::thing('pe', record::id($p))))`
--      —— `array::map` 的闭包参数是元素本身（record）。
--   2) 函数体内 `array::flatten` 只展开**一层**，三层遍历
--      `$pe<-pe_owner.in<-pe_owner.in<-pe_owner.in.id` 得到的是双层嵌套 `[[], [], …]`,
--      必须 `array::flatten(array::flatten(...))` 才能得到扁平记录数组。
--      顶层 `RETURN <expr>` 会自动扁平化，所以这个差异只在函数内出现。
-- 另：`array::complement(x, [NONE])` 滤不掉 NONE，改用 `array::filter(x, |$v| $v != NONE)`。
DEFINE FUNCTION fn::room_code($pe:record) {
    let $pe_id = type::thing('pe', record::id($pe));
    let $noun = (select value refno.TYPE from only $pe_id);
    let $uda_room = (select value v from type::thing('ATT_UDA', record::id($pe_id)).udas where u.NAME='/ROOM');
    return
    // EQUI可能有两层
    if ($noun == 'EQUI'|| $noun == 'STRU' || $noun == 'REST') && array::len($uda_room) == 0 {
        return array::distinct(array::flatten([
            array::filter(array::map(array::flatten($pe_id<-pe_owner.in.id),
                |$p| fn::room_num_of(type::thing('pe', record::id($p)))), |$v| $v != NONE),
            array::filter(array::map(array::flatten($pe_id<-pe_owner.in<-pe_owner.in.id),
                |$p| fn::room_num_of(type::thing('pe', record::id($p)))), |$v| $v != NONE)
        ]))
    } else if $noun == 'BRAN' && array::len($uda_room) == 0 {
        return array::filter(array::map(array::flatten($pe_id<-pe_owner.in.id),
            |$p| fn::room_num_of(type::thing('pe', record::id($p)))), |$v| $v != NONE)
    } else if $noun == 'SUPPO' {
        return array::distinct(array::filter(array::map(
            array::flatten(array::flatten($pe_id<-pe_owner.in<-pe_owner.in<-pe_owner.in.id)),
            |$p| fn::room_num_of(type::thing('pe', record::id($p)))), |$v| $v != NONE))
    } else if array::len($uda_room) == 0 {
        return array::filter(array::map([$pe_id],
            |$p| fn::room_num_of(type::thing('pe', record::id($p)))), |$v| $v != NONE)
    } else {
        return $uda_room
    }
};
