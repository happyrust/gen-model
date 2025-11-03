// 螺栓数量
remove function fn::flex_bolt_count;
DEFINE FUNCTION fn::flex_bolt_count($pe:record,$flan_width:float) {
    let $desp = $pe.refno.DESP;
    let $hvac_shape = fn::hvac_shape($pe);
    let $desp1 = $desp[1]?:0.0;
    let $desp2 = $desp[2]?:0.0;
    //return [$hvac_shape];
    return
    // (('[Prev_Type]' EQ 'DAMP') OR ('[Prev_Type]' EQ 'MESH')) AND (('[Next_Type]' EQ 'DAMP') OR ('[Next_Type]' EQ 'MESH'))
    select if $hvac_shape == 'RECT' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) {
        ((($desp[1] + $flan_width * 2 +75 ) / 150) + (($desp[2] + $flan_width * 2 + 75) / 150 -2)) * 4
    } else if $hvac_shape == 'CIRC' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) {
        (array::first(fn::strt_flan_length($pe)).length + 75)/ 150
    }
    //(('[Prev_Type]' EQ 'DAMP') OR ('[Prev_Type]' EQ 'MESH')) AND (('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH'))
    else if $hvac_shape == 'RECT' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) {
        ((($desp[1] + $flan_width * 2 + 75) / 150) + ((($desp[2] + $flan_width * 2 + 75) / 150 -2))) * 3
    }
    else if $hvac_shape == 'CIRC' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) {
        (($flan_width + 75)/ 150 )* 0.75
    }
    // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND (('[Next_Type]' EQ 'DAMP') OR ('[Next_Type]' EQ 'MESH'))
    else if $hvac_shape == 'RECT' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) && (!fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) {
        ((($desp[1] + $flan_width * 2 + 75) / 150 + 1.5) + ((($desp[2] + $flan_width)*2 + 75) / 150 -2)) * 2
    } else if $hvac_shape == 'CIRC' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) && (!fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) {
        ((array::first(fn::strt_flan_length($pe)).length + 75)/ 150 ) * 0.75
    }
    // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND (('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH'))
    else if $hvac_shape == 'RECT' && !fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH') && fn::b_next_type($pe,'DAMP') && fn::b_next_type($pe,'MESH') {
        ((($desp[1] + $flan_width * 2 + 75) / 150 + 1.5) + ((($desp[2] + $flan_width)*2 + 75) / 150 -2)) * 2
    } else if $hvac_shape == 'CIRC' && !fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH') && !fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH') {
        ((array::first(fn::strt_flan_length($pe)).length + 75)/ 150 ) / 2
    }
    else {
        0
    } as count from $pe;
};

// return fn::flex_bolt_count(pe:24381_58570,40)