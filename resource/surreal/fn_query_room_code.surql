REMOVE FUNCTION fn::room_code;
DEFINE FUNCTION fn::room_code($pe:record) {
    let $noun = $pe.refno.TYPE;
    let $id = return
    // First handle different entity types
    if ($noun == 'EQUI' || $noun == 'STRU' || $noun == 'REST') {
        array::flatten([$pe<-pe_owner.in.id, $pe<-pe_owner.in<-pe_owner.in.id])[0]
    } else if ($noun == 'BRAN') {
        $pe<-pe_owner.in.id
    } else if ($noun == 'SUPPO' || $noun == 'ZONE') {
        array::flatten($pe<-pe_owner.in<-pe_owner.in<-pe_owner.in.id)[0]
    } else {
        $pe
    };

    // Then check room relation
    let $res = select * from only $pe<-room_relate limit 1;
    if ($res == none) {
        return none;
    };

    let $prefix = string::split(string::slice($res.in.owner.name, 1), '-')[0];
    let $code =
    // Handle room number formatting
    if (string::len($res.room_num) == 4) {
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