/// 获取bran穿过的所有房间,返回panel的id
remove function fn::get_room_nodes;
define function fn::get_room_nodes($id:record) {
    let $room = select value in.refno from $id<-pe_owner.in<-room_relate;
    return array::distinct(array::flatten($room));
};

// 获取bran穿过的所有房间,返回房间号
remove function fn::get_room_names;
define function fn::get_room_names($id:record) {
    let $room = select value room_num from $id<-pe_owner.in<-room_relate;
    return array::distinct(array::flatten($room));
};
