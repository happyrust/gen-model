REMOVE FUNCTION fn::room_code;
DEFINE FUNCTION fn::room_code($pe:record) {
    let $noun = $pe.refno.TYPE;
    // SurrealDB 2.x：块的值是块内最后一条语句的值，而 `$id = …;` 这种赋值语句的值是 NONE，
    // 所以每个分支只放一个表达式，不在块里再赋值。
    // First handle different entity types
    let $id = if ($noun == 'EQUI' || $noun == 'STRU' || $noun == 'REST') {
        array::flatten([$pe<-pe_owner.in.id, $pe<-pe_owner.in<-pe_owner.in.id])[0]
    } else if ($noun == 'BRAN') {
        $pe<-pe_owner.in.id
    } else if ($noun == 'SUPPO' || $noun == 'ZONE') {
        array::flatten($pe<-pe_owner.in<-pe_owner.in<-pe_owner.in.id)[0]
    } else {
        $pe
    };

    // Then check room relation
    // 多归属时按归属强度取最强的一条，结果确定（ADR-010 §5）。
    let $res = fn::room_relate_of($pe);
    if ($res == none) {
        return none;
    };

    let $prefix = string::split(string::slice($res.in.owner.name, 1), '-')[0];
    // Handle room number formatting
    let $code = if (string::len($res.room_num) == 4) {
        $res.room_num
    } else if (string::len($res.room_num) == 5) {
        string::concat(string::slice($res.room_num, 0, 1), string::slice($res.room_num, 2))
    } else {
        none
    };

    // Return the formatted room code
    if ($code == none) {
        return none;
    };

    return string::concat($prefix, '-', $code);
};
