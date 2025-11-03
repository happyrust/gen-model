-- 仪控专业 设备表

remove function fn::yk_equi;
define function fn::yk_equi($pes: array<record>) {
    return select 
        fn::pe_to_refno(id) as id,
        refno.NAME as equi_name,
        fn::room_code($this.id)[0]?:'' as room_code,
        fn::get_world_pos($this.id)[0][2]?:0.0 as pos, // 坐标 z
        (->nearest_relate.dist)[0]?:0.0 as floor_height
    from $pes;
}

-- let $pes = [pe:24381_107713];
-- let $sbqd = fn::yk_sb($pes);
-- return $sbqd;

-- select refno,
-- refno.NAME as equi_name,
-- fn::room_code($this.id)[0] as room_code,
-- fn::get_world_pos($this.id)[0][2] as pos, // 坐标 z
-- (->nearest_relate.dist)[0] as floor_height
-- from $pe;