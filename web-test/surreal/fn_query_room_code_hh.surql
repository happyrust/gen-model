REMOVE FUNCTION fn::room_code;
DEFINE FUNCTION fn::room_code($pe:record) {
    let $noun = $pe.refno.TYPE;
    let $uda_room = (select value v from type::record('ATT_UDA', record::id($pe)).udas where u.NAME='/ROOM');
    return
    // EQUI可能有两层
    if ($noun == 'EQUI'|| $noun == 'STRU' || $noun == 'REST') && array::len($uda_room) == 0 {
        return array::distinct(
            (array::flatten([
        array::complement((select value (<-room_relate.room_num)[0] from array::flatten($pe<-pe_owner.in.id)),[NONE]),
        array::complement((select value (<-room_relate.room_num)[0] from array::flatten($pe<-pe_owner.in<-pe_owner.in.id)),[NONE])
        ])))
    } else if  $noun == 'BRAN' && array::len($uda_room) == 0 {
        return array::complement((select value (<-room_relate.room_num)[0] from $pe<-pe_owner.in.id), [none])
    } else if $noun == 'SUPPO' {
        return array::distinct(
            (array::flatten( array::complement((select value (<-room_relate.room_num)[0] from array::flatten($pe<-pe_owner.in<-pe_owner.in<-pe_owner.in.id)),[NONE])
        )))
    } else if array::len($uda_room) == 0 {
        return select value (<-room_relate.room_num)[0] from $pe
    } else {
        return $uda_room
    }
};
