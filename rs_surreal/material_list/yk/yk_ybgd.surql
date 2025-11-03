// 仪控 仪表管道
remove function fn::yk_ybgd;
define function fn::yk_ybgd($refnos: array<record>) {
    return select 
        fn::pe_to_refno(id) as id,
        name ,
        fn::find_gy_bran($this.id)[0][0]?:'' as pipe_name,
        fn::room_code($this.id)[0] as room_code
    from $refnos;
};

-- let $refnos = [pe:24381_147880];
-- let $ybgd = fn::yk_ybgd($refnos);
-- return $ybgd;

-- select id,
-- name ,
-- fn::find_pipe_bran($this.id)[0][0] as pipe_name,
-- fn::room_code($this.id)[0] as room_code
-- from pe:24381_147880