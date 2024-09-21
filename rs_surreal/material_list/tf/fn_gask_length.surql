// 垫片长度
remove FUNCTION fn::strt_gask_length;
DEFINE FUNCTION fn::strt_gask_length($pe:record,$flan_width:float) {
    let $pre_desp = fn::prev_pe($pe).refno.DESP?:[0.0];
    let $next_desp =  fn::next_pe($pe).refno.DESP?:[0.0];
    let $bran_4 = array::at(string::split($pe.owner.name?:'','-'),3)?:'';
    let $bran_b_6 = (array::at(string::split($pe.owner.name?:'','-'),3)?:'') == '6';
    let $hvac_shape = fn::hvac_shape($pe);
    return

    select if $hvac_shape == 'RECT' && array::first($pre_desp)[0] == 614823 && array::first($next_desp)[0] == 614823 && !$bran_b_6 {
        (array::first(fn::strt_flan_length($this.id)).length + $flan_width * 8) / 1000
    } else if $hvac_shape == 'CIRC' && array::first($pre_desp)[0] == 614823 && array::first($next_desp)[0] == 614823 && !$bran_b_6 {
        array::first(fn::strt_flan_length($this.id)).length
    }

    else if $hvac_shape == 'RECT' && array::first($pre_desp)[0] == 614823 && array::first($next_desp)[0] != 614823 && !$bran_b_6 {
        (array::first(fn::strt_flan_length($this.id)).length + $flan_width * 8) / 1000
    } else if $hvac_shape == 'CIRC' && array::first($pre_desp)[0] == 614823 && array::first($next_desp)[0] != 614823 && !$bran_b_6 {
        array::first(fn::strt_flan_length($this.id)).length / 2
    }
    // ('[Prev_Stype]' EQ 'FJFD') AND (('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && array::first($pre_desp)[0] == 614823 && !fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH') && !$bran_b_6 {
        (array::first(fn::strt_flan_length($this.id)).length + $flan_width * $flan_width * 8) / 1000
    } else if $hvac_shape == 'CIRC' && array::first($pre_desp)[0] == 614823 && !fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH') && !$bran_b_6 {
        array::first(fn::strt_flan_length($this.id)).length / 4
    }
    // ('[Prev_Stype]' EQ 'FJFD') AND ((('[Next_Type]' EQ 'DAMP') AND ([Next_Stype]' NE 'FJFD')) OR ('[Next_Type]' EQ 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && array::first($pre_desp)[0] == 614823 && (fn::b_next_type($pe,'DAMP') && array::first($next_desp)[0] == 614823) || (fn::b_next_type($pe,'DAMP')) && !$bran_b_6 {
        (array::first(fn::strt_flan_length($this.id)).length + $flan_width * $flan_width * 8) / 1000
    } else if $hvac_shape == 'CIRC' && array::first($pre_desp)[0] == 614823 && (fn::b_next_type($pe,'DAMP') && array::first($next_desp)[0] == 614823) || (fn::b_next_type($pe,'DAMP')) && !$bran_b_6 {
        array::first(fn::strt_flan_length($this.id)).length / 2
    }
    // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND ('[Next_Stype]' EQ 'FJFD') AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && array::first($next_desp)[0] == 614823 && !fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH') && !$bran_b_6 {
        (array::first(fn::strt_flan_length($this.id)).length + $flan_width * $flan_width * 8) / 1000
    } else if $hvac_shape == 'CIRC' && array::first($next_desp)[0] == 614823 && !fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH') && !$bran_b_6 {
        array::first(fn::strt_flan_length($this.id)).length / 4
    }
    // ((('[Prev_Type]' EQ 'DAMP') AND ('[Prev_Stype]' NE 'FJFD')) OR ('[Prev_Type]' EQ 'MESH')) AND ('[Next_Stype]' EQ 'FJFD') AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && ((fn::b_pre_type($pe,'DAMP') && array::first($pre_desp)[0] != 614823) || fn::b_pre_type($pe,'DAMP')) && array::first($next_desp)[0]== 614823 && !$bran_b_6 {
        (array::first(fn::strt_flan_length($this.id)).length + $flan_width * $flan_width * 8) / 1000
    } else if $hvac_shape == 'CIRC' && ((fn::b_pre_type($pe,'DAMP') && array::first($pre_desp)[0] != 614823) || fn::b_pre_type($pe,'DAMP')) && array::first($next_desp)[0]== 614823 && !$bran_b_6 {
        array::first(fn::strt_flan_length($this.id)).length / 2
    }
    //('[Prev_Stype]' NE 'FJFD') AND ('[Next_Stype]' EQ 'FJFD') AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && array::first($pre_desp)[0] != 614823 && array::first($next_desp)[0] == 614823 && !$bran_b_6 {
        (array::first(fn::strt_flan_length($this.id)).length + $fl_width * 8) / 1000 / 2
    } else if $hvac_shape == 'CIRC' && array::first($pre_desp)[0] != 614823 && array::first($next_desp)[0] == 614823 && !$bran_b_6 {
        array::first(fn::strt_flan_length($this.id)).length / 2
    }
    // ((('[Prev_Type]' EQ 'DAMP') AND ('[Prev_Stype]' NE 'FJFD')) OR ('[Prev_Type]' EQ 'MESH')) AND ((('[Next_Type]' EQ 'DAMP') AND ('[Next_Stype]' NE 'FJFD')) OR ('[Next_Type]' EQ 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && ((fn::b_pre_type($pe,'DAMP') && array::first($pre_desp)[0] != 614823) || !fn::b_pre_type($pe,'MESH')) && ((fn::b_next_type($pe,'DAMP') && array::first($next_desp)[0] != 614823) || fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        (array::first(fn::strt_flan_length($this.id)).length + $fl_width * 8) / 1000
    } else if $hvac_shape == 'CIRC' && ((fn::b_pre_type($pe,'DAMP') && array::first($pre_desp)[0] != 614823) || !fn::b_pre_type($pe,'MESH')) && ((fn::b_next_type($pe,'DAMP') && array::first($next_desp)[0] != 614823) || fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        array::first(fn::strt_flan_length($this.id)).length
    }
    // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND ((('[Next_Type]' EQ 'DAMP') AND ('[Next_Stype]' NE 'FJFD')) OR ('[Next_Type]' EQ 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && (!fn::b_pre_type($pe,'DAMP') && (!fn::b_pre_type($pe,'MESH')) && ((!fn::b_next_type($pe,'DAMP') && array::first($next_desp)[0] != 614823)) || fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        ((array::first(fn::strt_flan_length($this.id)).length + $fl_width * 8) / 1000) * 0.75
    } else if $hvac_shape == 'CIRC' && (!fn::b_pre_type($pe,'DAMP') && (!fn::b_pre_type($pe,'MESH')) && ((!fn::b_next_type($pe,'DAMP') && array::first($next_desp)[0] != 614823)) || fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        array::first(fn::strt_flan_length($this.id)).length / 2 * 0.75
    }
    // ((('[Prev_Type]' EQ 'DAMP') AND ('[Prev_Stype]' NE 'FJFD')) OR ('[Prev_Type]' EQ 'MESH')) AND (('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && ((fn::b_pre_type($pe,'DAMP') && array::first($pre_desp)[0] != 614823) || fn::b_pre_type($pe,'MESH')) && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        ((array::first(fn::strt_flan_length($this.id)).length + $fl_width * 8) / 1000) * 0.75
    } else if $hvac_shape == 'CIRC' && ((fn::b_pre_type($pe,'DAMP') && array::first($pre_desp)[0] != 614823) || fn::b_pre_type($pe,'MESH')) && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        array::first(fn::strt_flan_length($this.id)).length * 0.75
    }
    // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND (('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH')) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        ((array::first(fn::strt_flan_length($this.id)).length + $fl_width * 8) / 1000) / 2
    } else if $hvac_shape == 'CIRC' && (!fn::b_pre_type($pe,'DAMP') && !fn::b_pre_type($pe,'MESH')) && (!fn::b_next_type($pe,'DAMP') && !fn::b_next_type($pe,'MESH')) && !$bran_b_6 {
        array::first(fn::strt_flan_length($this.id)).length / 2
    }
    else {
        ''
    } as size from $pe
};


// return fn::strt_gask_length(pe:24381_58570,0)