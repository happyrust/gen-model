REMOVE FUNCTION fn::room_code;
DEFINE FUNCTION fn::room_code($pe:record) {
    let $noun = $pe.refno.TYPE;
    let $uda_room = (select value v from type::thing('ATT_UDA', record::id($pe)).udas where u.NAME='/ROOM');
    return
    // EQUI可能有两层
    if ($noun == 'EQUI'|| $noun == 'STRU' || $noun == 'REST') && array::len($uda_room) == 0 {
        return array::distinct(
            (array::flatten([
        array::complement((select value fn::room_num_of($this) from array::flatten($pe<-pe_owner.in.id)),[NONE]),
        array::complement((select value fn::room_num_of($this) from array::flatten($pe<-pe_owner.in<-pe_owner.in.id)),[NONE])
        ])))
    } else if  $noun == 'BRAN' && array::len($uda_room) == 0 {
        return array::complement((select value fn::room_num_of($this) from $pe<-pe_owner.in.id), [none])
    } else if $noun == 'SUPPO' {
        return array::distinct(
            (array::flatten( array::complement((select value fn::room_num_of($this) from array::flatten($pe<-pe_owner.in<-pe_owner.in<-pe_owner.in.id)),[NONE])
        )))
    } else if array::len($uda_room) == 0 {
        return select value fn::room_num_of($this) from $pe
    } else {
        return $uda_room
    }
};