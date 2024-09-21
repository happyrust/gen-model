
remove function fn::is_valve_support;
DEFINE FUNCTION fn::is_valve_support($pe:record) {
    let $pre = fn::prev_pe($pe);
    if $pre.TYPE != 'ATTA' {
        return '否';
    };
    if $pre.refno.POS == null || $pe.refno.POS == null {
        return '否';
    };
    // 使用 vector::distance::euclidean() 来计算距离
    let $distance = vector::distance::euclidean($pre.refno.POS, $pe.refno.POS);

    // 定义一个小的阈值来判断位置是否"相等"
    let $threshold = 0.01;

    return if $distance < $threshold {
        '是'
    } else {
        '否'
    };
};


remove function fn::get_valve_param;
DEFINE FUNCTION fn::get_valve_param($catr:record, $param_name:string) {
    LET $is_r = string::slice($catr.name?:'', 4, 1) == "R";
    RETURN 
        if $is_r && $param_name == 'weight'{
            math::fixed($catr.refno.PARA[14]?:0, 2)
        } else if $is_r && $param_name == 'x' {
            math::fixed($catr.refno.PARA[11]?:0, 2)
        } else if $is_r && $param_name == 'y' {
            math::fixed($catr.refno.PARA[12]?:0, 2)
        } else if $is_r && $param_name == 'z' {
            math::fixed($catr.refno.PARA[13]?:0, 2)
        } else if !$is_r && $param_name == 'weight' {
            math::fixed($catr.refno.PARA[10]?:0, 2)
        } else if !$is_r && $param_name == 'x' {
            math::fixed($catr.refno.PARA[7]?:0, 2)
        } else if !$is_r && $param_name == 'y' {
            math::fixed($catr.refno.PARA[8]?:0, 2)
        } else if !$is_r && $param_name == 'z' {
            math::fixed($catr.refno.PARA[9]?:0, 2)
        };
};

remove function fn::gy_valve;
DEFINE FUNCTION fn::gy_valve($pes: array<record>) {
    return select 
        fn::pe_to_refno(id) as id,
        string::slice(fn::default_name(id),1) as valv_name, // 阀门位号
        fn::room_code(id)[0]?:'' as room_code, // 房间号
        string::split(string::slice(array::at(->pe_owner.out.name,0),1),'-')[0] as valv_belong, // 阀门归属
        refno.SPRE.refno.CATR.refno.PARA[1]?:0 * 2 as valv_length, // 阀门长度
        if refno.SPRE.refno.CATR != NONE { fn::get_valve_param(refno.SPRE.refno.CATR, 'weight') } else { 0 } as valv_weight, // 阀门重量
        if refno.SPRE.refno.CATR != NONE { fn::get_valve_param(refno.SPRE.refno.CATR, 'x') } else { 0 } as valv_x, // 阀门重心X
        if refno.SPRE.refno.CATR != NONE { fn::get_valve_param(refno.SPRE.refno.CATR, 'y') } else { 0 } as valv_y, // 阀门重心Y
        if refno.SPRE.refno.CATR != NONE { fn::get_valve_param(refno.SPRE.refno.CATR, 'z') } else { 0 } as valv_z, // 阀门重心Z
        fn::is_valve_support(id) as valv_supp // 阀门支架 
    from $pes where noun == 'VALV';
};

-- let $pe = [pe:24383_66745];
-- return fn::gy_valve($pe);