// 设备清单
remove function fn::gy_equip;
DEFINE FUNCTION fn::gy_equip($refnos: array<record>) {
    return select
        fn::pe_to_refno(id) as id,
        string::slice(refno.NAME?:' ',1) as name, // 设备位号
        fn::room_code($this.id)[0] as room_code,  // 房间号
        fn::default_names(array::flatten(array::union(<-pe_owner[where in.noun='NOZZ'].in,  <-pe_owner.in<-pe_owner[where in.noun='NOZZ'].in)))
            as nozz_name, // 管口号
        array::flatten(array::union(<-pe_owner[where in.noun='NOZZ'].in.refno, <-pe_owner.in<-pe_owner[where in.noun='NOZZ'].in.refno)).POS as nozz_pos, // 管口坐标
        array::flatten(array::union(<-pe_owner[where in.noun='NOZZ'].in,  <-pe_owner.in<-pe_owner[where in.noun='NOZZ'].in)) as nozz_cref // 相连管道编号
    from $refnos where noun == 'EQUI';
};

//待测试参考号
// let $refno = [pe:24381_57828,pe:24383_76416];
// let $equip_list = fn::gy_equip($refno);
// 24381_57828  24383_76416
