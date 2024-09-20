// let $refno = [pe:24383_84088];

remove function fn::dq_bran;
DEFINE FUNCTION fn::dq_bran($pes: array<record>) {
    RETURN SELECT
        fn::pe_to_refno(refno) as id,
        fn::get_unit_number(id) AS num, // 机组号
        fn::get_project_number(id) AS project_num, // 子项号
        fn::get_major_type(id) AS major, // 专业
        fn::room_code(id)[0]?:'' AS room_code, // 房间号
        string::slice(refno.NAME?: ' ', 1) AS name, // 托盘段号
        refno.HPOS[2]?:0.0 AS pos, // 托盘标高
        fn::get_tray_type(id) AS bran_type, // 托盘类型
        '碳钢Q235' AS material, // 材质
        fn::get_uda_value(id, '/BranWidth')?:0.0 AS width, // 托盘宽度
        fn::get_uda_value(id, '/BranHigh') ?:0.0 AS height, // 托盘高度
        fn::get_uda_value(id, '/BranWidth')?:0.0 * fn::get_uda_value(id, '/BranHigh')?:0.0 AS size_num, // 规格型号
        'Y' AS b_painting, // 是否刷漆
        string::split(refno.NAME?:'- ', '-')[1] AS painting_color, // 刷漆颜色
        fn::get_cover_info(id) AS b_cover, // 有无盖板
        if refno.DESC == NONE  { '无' }  else  { '是' }  AS b_partition, // 有无隔板
        refno.DESC AS partition_num, 
        fn::get_spre_info(id) AS spre,
        fn::get_catr_info(id) AS catr,
        fn::get_orientation(id) AS horizontal_or_vertical
    FROM $pes;
};





-- define function fn::dq_dz($pes:array<record>){
--     return select refno,
--         string::slice(fn::find_ancestor_type($this.id,"SITE")[0].refno.NAME,1,1) as num, // 机组号
--         string::slice(string::split(fn::find_ancestor_type($this.id,"SITE")[0].refno.NAME,'-')[0],2) as project_num, //子项号
--         if string::contains(fn::find_ancestor_type($this.id,"SITE")[0].refno.NAME,'MCT')
--         || string::contains(fn::find_ancestor_type($this.id,"ZONE")[0].refno.NAME,'MSUP')  { '主托盘' } else { '次托盘' } as major,//专业
--         fn::room_code($this.id)[0] as room_code,
--         string::slice(refno.NAME,1) as name,// 托盘段号
--         refno.HPOS[2] as pos, // 托盘标高
--         //<-pe_owner[where in.noun!='ATTA'] order by order_num,

--         if string::contains(string::uppercase((select value in from only (select * from <-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.SPRE.refno.NAME),'LADDER') { '梯架' } else if string::contains(string::uppercase((select value in from only (select * from <-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.SPRE.refno.NAME),'TROUGH') { '实底托盘' } else if string::contains(string::uppercase((select value in from only (select * from <-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.SPRE.refno.NAME),'VENTILATED') { '带孔托盘' } else if string::contains(string::uppercase((select value in from only (select * from <-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.SPRE.refno.NAME),'RISER') { '竖梯' } else if string::contains(string::uppercase((select value in from only (select * from <-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.SPRE.refno.NAME),'DIVIDER') { '分隔板' } as children, // 托盘类型
-- '碳钢Q235' as material, // 材质
-- array::first(array::first((select value (select value v from only udas.* where u.NAME=='/BranWidth' limit 1) from type::thing("ATT_UDA", meta::id(id))))) as width, // 托盘宽度
-- array::first(array::first((select value (select value v from only udas.* where u.NAME=='/BranHigh' limit 1) from type::thing("ATT_UDA", meta::id(id))))) as height, // 托盘高度
--         array::first(array::first((select value (select value v from only udas.* where u.NAME=='/BranWidth' limit 1) from type::thing("ATT_UDA", record::id(id))))) * array::first(array::first((select value (select value v from only udas.* where u.NAME=='/BranHigh' limit 1) from type::thing("ATT_UDA", record::id(id))))) as size_num, // 规格型号
--         'Y' as b_painting, // 是否刷漆
--         string::split(refno.NAME,'-')[1] as painting_color,
--         (select value in from only (select * from <-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.DESP as b_cover, //有无盖板
--         if refno.DESC == NONE { '无' } else { '是' } as b_partition, // 有无隔板
--         refno.DESC as partition_num,
--         (select value in from only (select * from <-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.SPRE.refno.NAME as spre,
--         (select value in from only (select * from <-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.SPRE.refno.CATR.refno.NAME as catr,
--         if string::contains(string::uppercase((select value in from only (select * from <-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.SPRE.refno.NAME),'RISER') { '竖向' } else if string::contains(string::uppercase((select value in from only (select * from <-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.SPRE.refno.NAME),'DIVIDER') { '竖向' } else { '水平' } as horizontal_or_vertical
--     from $pes;
-- };

-- let $pe = [pe:24383_84088];
-- return fn::dq_dz($pe);   