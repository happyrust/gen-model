remove FUNCTION fn::dq_bran_type;
DEFINE FUNCTION fn::dq_bran_type($pe:record) {
    let $first_spre = (select value in from only (select * from $pe<-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.SPRE.refno.NAME;
    return
    if $first_spre != NONE && string::contains(string::uppercase($first_spre),'LADDER') { '梯架' }
    else if $first_spre!= NONE && string::contains(string::uppercase($first_spre),'TROUGH') { '实底托盘' }
    else if $first_spre!= NONE && string::contains(string::uppercase($first_spre),'VENTILATED') { '带孔托盘' }
    else if $first_spre!= NONE && string::contains(string::uppercase($first_spre),'RISER') { '竖梯' }
    else if $first_spre!= NONE && string::contains(string::uppercase($first_spre),'DIVIDER') { '分隔板' } else { '' }
};

remove FUNCTION fn::dq_horizontal_or_vertical;
DEFINE FUNCTION fn::dq_horizontal_or_vertical($pe:record) {
    let $first_spre = (select value in from only (select * from $pe<-pe_owner[where in.noun != 'ATTA'] order by order_num) limit 1).refno.SPRE.refno.NAME;
    return
    if $first_spre != NONE && (string::contains(string::uppercase($first_spre),'RISER') || string::contains(string::uppercase($first_spre),'DIVIDER')) {
        '竖向'
    } else {
        '水平'
    }
};

remove FUNCTION fn::get_unit_number;
DEFINE FUNCTION fn::get_unit_number($id:record) {
    RETURN string::slice(fn::find_ancestor_type($id, "SITE")[0].refno.NAME, 1, 1);
};

remove FUNCTION fn::get_project_number;
DEFINE FUNCTION fn::get_project_number($id:record) {
    RETURN string::slice(string::split(fn::find_ancestor_type($id, "SITE")[0].refno.NAME, '-')[0], 2);
};

remove FUNCTION fn::get_major_type;
DEFINE FUNCTION fn::get_major_type($id:record) {
    LET $site_name = fn::find_ancestor_type($id, "SITE")[0].refno.NAME ?: '';
    LET $zone_name = fn::find_ancestor_type($id, "ZONE")[0].refno.NAME ?: '';
    RETURN IF string::contains($site_name, 'MCT') || string::contains($zone_name, 'MSUP') { '主托盘' } ELSE { '次托盘' };
};

remove FUNCTION fn::get_tray_type;
DEFINE FUNCTION fn::get_tray_type($pe:record) {
    LET $first_child = (SELECT VALUE in FROM ONLY (SELECT * FROM $pe<-pe_owner[WHERE in.noun != 'ATTA'] ORDER BY order_num) LIMIT 1);
    LET $spre_name = string::uppercase($first_child.refno.SPRE.refno.NAME ?: '');
    
    RETURN 
        IF string::contains($spre_name, 'LADDER') { '梯架' }
        ELSE IF string::contains($spre_name, 'TROUGH') { '实底托盘' }
        ELSE IF string::contains($spre_name, 'VENTILATED') { '带孔托盘' }
        ELSE IF string::contains($spre_name, 'RISER') { '竖梯' }
        ELSE IF string::contains($spre_name, 'DIVIDER') { '分隔板' }
        ELSE { '未知类型' };
};

remove FUNCTION fn::get_uda_value;
DEFINE FUNCTION fn::get_uda_value($pe:record, $uda_name:string) {
    RETURN (
        SELECT VALUE (
            SELECT VALUE v FROM ONLY udas.* WHERE u.NAME == $uda_name LIMIT 1
        ) FROM type::thing("ATT_UDA", record::id($pe.id))
    )[0][0];
};

remove FUNCTION fn::get_cover_info;
DEFINE FUNCTION fn::get_cover_info($pe:record) {
    RETURN if (SELECT VALUE in FROM ONLY (SELECT * FROM $pe<-pe_owner[WHERE in.noun != 'ATTA'] ORDER BY order_num) LIMIT 1).refno.DESP[2] == 1 { '是' } else { '否'};
};

remove FUNCTION fn::get_spre_info;
DEFINE FUNCTION fn::get_spre_info($pe:record) {
    RETURN (SELECT VALUE in FROM ONLY (SELECT * FROM $pe<-pe_owner[WHERE in.noun != 'ATTA'] ORDER BY order_num) LIMIT 1).refno.SPRE.refno.NAME;
};

remove FUNCTION fn::get_catr_info;
DEFINE FUNCTION fn::get_catr_info($pe:record) {
    RETURN (SELECT VALUE in FROM ONLY (SELECT * FROM $pe<-pe_owner[WHERE in.noun != 'ATTA'] ORDER BY order_num) LIMIT 1).refno.SPRE.refno.CATR.refno.NAME;
};

remove FUNCTION fn::get_orientation;
DEFINE FUNCTION fn::get_orientation($pe:record) {
    LET $spre_name = string::uppercase(fn::get_spre_info($pe)?:'');
    RETURN IF string::contains($spre_name, 'RISER') || string::contains($spre_name, 'DIVIDER') { '竖向' } ELSE { '水平' };
};


// return fn::dq_horizontal_or_vertical(pe:24383_84088)

