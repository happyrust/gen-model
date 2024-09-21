// 螺栓数量
remove function fn::tape_bolt_count;
DEFINE FUNCTION fn::tape_bolt_count($pe:record) {
    let $hvac_shape = fn::hvac_shape($pe);
    let $width = $pe.refno.DESP[1]?:0.0;
    let $width_2 = $pe.refno.DESP[3]?:0.0;
    let $height = $pe.refno.DESP[2]?:0.0;
    let $height_2 = $pe.refno.DESP[4]?:0.0;
    let $flan_width = $pe.refno.DESP[20]?:0.0;
    let $flan_width_2 = $pe.refno.DESP[23]?:0.0;
    let $flan_length = <float>((string::split(fn::tape_flan_length($pe),',')[0])?:'0');

    let $pre_length =
    // ('[Prev_Type]' EQ 'DAMP') OR ('[Prev_Type]' EQ 'MESH')
    if $hvac_shape == 'RECT' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) {
        ((($width + $flan_width * 2 + 75 ) / 150) + (($height + $flan_width * 2 + 75) / 150 ) - 2) *2
    }
    // ('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')
    else if $hvac_shape == 'RECT' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) {
        (($width + $flan_width * 2 + 75 ) / 150) + (($height + $flan_width * 2 + 75) / 150 ) - 2
    }
    // ('[Prev_Type]' EQ 'DAMP') OR ('[Prev_Type]' EQ 'MESH')
    else if $hvac_shape == 'CIRC' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) {
        (<float>$flan_length[0].length + 75) / 150
    }
    // ('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')
    else if $hvac_shape == 'CIRC' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) {
        (<float>$flan_length[0].length + 75) / 150 / 2
    } else {
        0
    };

    let $next_length =
    // ('[Next_Type]' EQ 'DAMP') OR ('[Next_Type]' EQ 'MESH')
    if $hvac_shape == 'RECT' && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) {
        ((($width + $flan_width * 2 + 75 ) / 150) + (($height + $flan_width * 2 + 75) / 150 ) - 2) *2
    }
    // ('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH')
    else if $hvac_shape == 'RECT' && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) {
        (($width + $flan_width_2 * 2 + 75 ) / 150) + (($height_2 + $flan_width_2 * 2 + 75) / 150 ) - 2
    }
    // ('[Next_Type]' EQ 'DAMP') OR ('[Next_Type]' EQ 'MESH')
    else if $hvac_shape == 'CIRC' && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) {
        (<float>$flan_length[0].length + 75) / 150
    }
    //('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH')
    else if $hvac_shape == 'CIRC' && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) {
        (<float>$flan_length[0].length + 75) / 150 / 2
    } else {
        0
    };

    return string::concat(<string>math::fixed($pre_length,2),',',<string>math::fixed($next_length,2))
};


// return fn::tape_gask_length(pe:24381_76894)
