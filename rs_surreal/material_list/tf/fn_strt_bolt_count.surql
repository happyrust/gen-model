// 螺栓数量
remove function fn::strt_bolt_count;
DEFINE FUNCTION fn::strt_bolt_count($pe:record,$flan_width:float) {
    let $desp = $pe.refno.DESP;
    let $bran_4 = array::at(string::split($pe.owner.name?:'','-'),3)?:'';
    let $bran_b_6 = (array::at(string::split($pe.owner.name?:'','-'),3)?:'') == '6';

    let $desp1 = $pe.refno.DESP[1]?:0.0;
    let $desp2 = $pe.refno.DESP[2]?:0.0;
    let $length = array::first(fn::strt_flan_length($pe)).length?:0.0;

    let $hvac_shape = fn::hvac_shape($pe);
    //return [$hvac_shape];
    return
    // (('[Prev_Type]' EQ 'DAMP') OR ('[Prev_Type]' EQ 'MESH')) AND (('[Next_Type]' EQ 'DAMP') OR ('[Next_Type]' EQ 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    select if $hvac_shape == 'RECT' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        ((($desp[1] + $flan_width) / 150 + 1.5) + (($desp[2] + $flan_width) / 150 + 1.5)) * 4
    } else if $hvac_shape == 'CIRC' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        ( $length + 75)/ 150
    }
    //(('[Prev_Type]' EQ 'DAMP') OR ('[Prev_Type]' EQ 'MESH')) AND (('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        ((($desp[1] + $flan_width * 2) / 150 + 1.5) + ((($desp[2] + $flan_width)*2) / 150 + 1.5)) * 3
    } else if $hvac_shape == 'CIRC' && (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        (($length + 75)/ 150 )* 0.75
    }
    // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND (('[Next_Type]' EQ 'DAMP') OR ('[Next_Type]' EQ 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        ((($desp[1] + $flan_width * 2) / 150 + 1.5) + ((($desp[2] + $flan_width)*2) / 150 + 1.5)) * 3
    } else if $hvac_shape == 'CIRC' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        (($length + 75)/ 150 )* 0.75
    }
    // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND (('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        ((($desp[1] + $flan_width * 2) / 150 + 1.5) + ((($desp[2] + $flan_width)*2) / 150 + 1.5)) * 2
    } else if $hvac_shape == 'CIRC' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        (($length + 75)/ 150 ) / 2
    } else { 0 } as count from $pe;
};

// return fn::strt_bolt_count(pe:24381_58570,40) // 330 / 150 + 1.5 + 230 / 150 +1.5