// 垫片长度
remove function fn::trns_gask_length_pre;
DEFINE FUNCTION fn::trns_gask_length_pre($pe:record) {
    let $flan_length = array::first(fn::flex_flan_length($pe)).length?:0.0;
    let $flan_width = $pe.refno.DESP[20]?:0.0;

    let $pre_b_damp = array::first(fn::b_pre_type($pe,'DAMP'));
    let $pre_b_mesh = array::first(fn::b_pre_type($pe,'MESH'));

    let $height2 = string::e3d_dehash($pe.refno.DESP[4]);
    let $height = $pe.refno.DESP[2]?:0.0;
    let $height_str = string::e3d_dehash($height);

    return
    select if $height2 == 'DIAM' && ($pre_b_damp || $pre_b_mesh) {
        $flan_length + ($flan_width * 4) / 1000
    }
    else if $height2 == 'DIAM' && (!$pre_b_damp && !$pre_b_mesh)  {
        $flan_length + ($flan_width * 4) / 1000 / 2
    }

    else if $height_str == 'DIAM' && ($pre_b_damp || $pre_b_mesh)  {
            $flan_length
    }
    else if $height_str == 'DIAM' && (!$pre_b_damp && !$pre_b_mesh)  {
            $flan_length / 2
    }
    else {
        ''
    } as size from $pe
};

remove function fn::trns_gask_length_next;
DEFINE FUNCTION fn::trns_gask_length_next($pe:record) {
    let $flan_length = array::first(fn::flex_flan_length($pe)).length?:0.0;
    let $flan_width = $pe.refno.DESP[20]?:0.0;
    let $flan_width2 = $pe.refno.DESP[23]?:0.0;

    let $next_b_damp = array::first(fn::b_next_type($pe,'DAMP'));
    let $next_b_mesh = array::first(fn::b_next_type($pe,'MESH'));

    let $height2 = string::e3d_dehash($pe.refno.DESP[4]);
    let $height = $pe.refno.DESP[2]?:0.0;
    let $height_str = string::e3d_dehash($height);

    return
    select if $height2 == 'DIAM' && ($next_b_damp || $next_b_mesh) {
        $flan_length
    }
    else if $height2 == 'DIAM' && (!$next_b_damp && !$next_b_mesh)  {
        $flan_length / 2
    }

    else if $height_str == 'DIAM' && ($next_b_damp || $next_b_mesh)  {
            ($flan_length + $flan_width2 * 4) / 1000
    }
    else if $height_str == 'DIAM' && (!$next_b_damp && !$next_b_mesh)  {
            ($flan_length + $flan_width2 * 4) / 1000 / 2
    }
    else {
        ''
    } as size from $pe

};

remove function fn::trns_gask_size;
DEFINE FUNCTION fn::trns_gask_size($pe:record) {
    return string::concat(array::first(fn::trns_gask_size_pre($pe)).size,',',array::first(fn::trns_gask_size_next($pe)).size);
};


// return fn::flex_gask_size(pe:24381_57347)