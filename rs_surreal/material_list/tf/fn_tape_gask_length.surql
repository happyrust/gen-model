// 垫片长度
remove function fn::tape_gask_length;
DEFINE FUNCTION fn::tape_gask_length($pe:record) {
    let $hvac_shape = fn::hvac_shape($pe);
    let $flan_width = $pe.refno.DESP[20]?:0.0;
    let $flan_width_2 = $pe.refno.DESP[23]?:0.0;
    let $flan_length = string::split(array::first(fn::tape_flan_length($pe)).length,',')?:'';
    //return array::first(fn::tape_flan_length($pe)).length;
    //return $flan_length;
    let $pre_length =
    // ('[Prev_Type]' EQ 'DAMP') OR ('[Prev_Type]' EQ 'MESH')
    if $hvac_shape == 'RECT' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) {
        (<float>$flan_length[0] + ($flan_width / 1000 ) * 8) / 2
    }
    // ('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')
    else if $hvac_shape == 'RECT' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) {
        (<float>$flan_length[0] + ($flan_width / 1000) * 8) / 4
    }
    // ('[Prev_Type]' EQ 'DAMP') OR ('[Prev_Type]' EQ 'MESH')
    else if $hvac_shape == 'CIRC' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) {
        <float>$flan_length[0].length / 2
    }
    // ('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')
    else if $hvac_shape == 'CIRC' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) {
        <float>$flan_length[0] / 4
    } else {
        0
    };

    let $next_length =
    // ('[Next_Type]' EQ 'DAMP') OR ('[Next_Type]' EQ 'MESH')
    if $hvac_shape == 'RECT' && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) {
        (<float>$flan_length[1] + ($flan_width / 1000 ) * 8) / 2
    }
    // ('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH')
    else if $hvac_shape == 'RECT' && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) {
        (<float>$flan_length[1] + ($flan_width_2 / 1000) * 8) / 4
    }
    // ('[Next_Type]' EQ 'DAMP') OR ('[Next_Type]' EQ 'MESH')
    else if $hvac_shape == 'CIRC' && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) {
        <float>$flan_length[1] / 2
    }
    //('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH')
    else if $hvac_shape == 'CIRC' && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) {
        <float>$flan_length[1] / 4
    } else {
        0
    };

    return string::replace(string::concat(<string>$pre_length,',',<string>$next_length),'f','')
};


// return fn::tape_gask_length(pe:24381_76894)
