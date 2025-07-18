// let $refno = [pe:24383_84088];

remove function fn::dq_bran;
DEFINE FUNCTION fn::dq_bran($pes: array<record>) {
    RETURN SELECT
        fn::pe_to_refno(refno) as id,
        fn::get_unit_number(id) AS num, // 机组号
        fn::get_project_number(id) AS project_num, // 子项号
        fn::get_major_type(id) AS major, // 专业
        (fn::room_code(id)?:[])[0]?:'' AS room_code, // 房间号
        string::slice(refno.NAME?:' ', 1) AS name, // 托盘段号
        (refno.HPOS?:[])[2]?:0.0 AS pos, // 托盘标高
        fn::get_tray_type(id) AS bran_type, // 托盘类型
        '碳钢Q235' AS material, // 材质
        (fn::get_uda_value(id, '/BranWidth')?:[])[0]?:0.0 AS width, // 托盘宽度
        (fn::get_uda_value(id, '/BranHigh')?:[])[0]?:0.0 AS height, // 托盘高度
        (fn::get_uda_value(id, '/BranWidth')?:[])[0]?:0.0 * (fn::get_uda_value(id, '/BranHigh')?:[])[0]?:0.0 AS size_num, // 规格型号
        'Y' AS b_painting, // 是否刷漆
        (string::split(refno.NAME?:'- ', '-')?:[])[1]?:'' AS painting_color, // 刷漆颜色
        fn::get_cover_info(id) AS b_cover, // 有无盖板
        if refno.DESC == NONE  { '无' }  else  { '是' }  AS b_partition, // 有无隔板
        refno.DESC?:'' AS partition_num,
        fn::get_spre_info(id) AS spre,
        fn::get_catr_info(id) AS catr,
        fn::get_orientation(id) AS horizontal_or_vertical
    FROM $pes;
};