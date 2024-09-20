// 螺栓数量
remove function fn::bend_bolt_count;
DEFINE FUNCTION fn::bend_bolt_count($pe:record) {
    let $desp = $pe.refno.DESP;
    let $pre_desp = fn::prev_pe($pe).refno.DESP?:[0.0];
    let $next_desp = fn::next_pe($pe).refno.DESP?:[0.0];
    let $flan_length = <float>array::first(fn::flex_flan_length($pe)).length;
    let $flan_width = $pe.refno.DESP[20];
    let $bran_4 = array::at(string::split($pe.owner.name,'-')?:[],3)?:'';
    let $hvac_shape = fn::hvac_shape($pe);

    let $pre_b_damp = fn::b_pre_type($pe,'DAMP');
    let $pre_b_mesh = fn::b_pre_type($pe,'MESH');
    let $next_b_damp = fn::b_next_type($pe,'DAMP');
    let $next_b_mesh = fn::b_next_type($pe,'MESH');
    //return if $hvac_shape == 'RECT' && !$pre_b_damp && !$pre_b_mesh && !$next_b_damp && !$next_b_mesh && $bran_4 != '6' { 1 } else { 0 } ;
    //return [$hvac_shape == 'RECT' , !$pre_b_damp , !$pre_b_mesh , !$next_b_damp , !$next_b_mesh , $bran_4 != '6'];
    return
    // (('[Prev_Type]' EQ 'DAMP') OR ('[Prev_Type]' EQ 'MESH')) AND (('[Next_Type]' EQ 'DAMP') OR ('[Next_Type]' EQ 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    select if $hvac_shape == 'RECT' && ($pre_b_damp || $pre_b_mesh) && ($next_b_damp || $next_b_mesh) && $bran_4 != '6' {
        ((($desp[1] + $flan_width * 2 +75 ) / 150) + (($desp[2] + $flan_width * 2 + 75) / 150 -2)) * 4
    } else if $hvac_shape == 'CIRC' && ($pre_b_damp || $pre_b_mesh) && ($next_b_damp || $next_b_mesh) && $bran_4 != '6' {
        (array::first(fn::strt_flan_length($pe)).length + 75)/ 150
    }
    // (('[Prev_Type]' EQ 'DAMP') OR ('[Prev_Type]' EQ 'MESH')) AND (('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && ($pre_b_damp || $pre_b_mesh) && (!$next_b_damp && !$next_b_mesh) && $bran_4 != '6' {
        ((($desp[1] + $flan_width * 2 + 75) / 150) + ((($desp[2] + $flan_width * 2 + 75) / 150 -2))) * 3
    }
    else if $hvac_shape == 'CIRC' && ($pre_b_damp || $pre_b_mesh) && (!$next_b_damp && !$next_b_mesh) && $bran_4 != '6' {
        (($flan_width + 75)/ 150 )* 0.75
    }
    // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND (('[Next_Type]' EQ 'DAMP') OR ('[Next_Type]' EQ 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && (!$pre_b_damp && !$pre_b_mesh) && ($next_b_damp || $next_b_mesh) && !$bran_4 != '6' {
        ((($desp[1] + $flan_width * 2 + 75) / 150 + 1.5) + ((($desp[2] + $flan_width * 2 + 75) / 150 -2))) * 3
    } else if $hvac_shape == 'CIRC' && (!$pre_b_damp && !$pre_b_mesh) && ($next_b_damp || $next_b_mesh) && !$bran_4 != '6' {
        ((array::first(fn::strt_flan_length($pe)).length + 75)/ 150 ) * 0.75
    }
    // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND (('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && (!$pre_b_damp && !$pre_b_mesh) && (!$next_b_damp && !$next_b_mesh) && !$bran_4 != '6' {
        ((($desp[1] + $flan_width * 2 + 75) / 150 ) + ((($desp[2] + $flan_width * 2 + 75) / 150 -2))) * 2
        //'0'
    } else if $hvac_shape == 'CIRC' && (!$pre_b_damp && !$pre_b_mesh) && (!$next_b_damp && !$next_b_mesh) && !$bran_4 != '6'  {
        ((array::first(fn::strt_flan_length($pe)).length + 75)/ 150 ) / 2
    }
    else {
        0
    } as count from $pe;
};

// return fn::bend_bolt_count(pe:24381_58571)