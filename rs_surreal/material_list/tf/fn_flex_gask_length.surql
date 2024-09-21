// 垫片类型
remove function fn::flex_gask_length_pre;
DEFINE FUNCTION fn::flex_gask_length_pre($pe:record) {
    let $pre_desp = fn::prev_pe($pe).refno.DESP?:[0.0];
    let $next_desp =  fn::next_pe($pe).refno.DESP?:[0.0];
    let $flan_length = array::first(fn::flex_flan_length($pe)).length?:0.0;
    let $flan_width = $pe.refno.DESP[20]?:0.0;
    let $bran_4 = array::at(string::split($pe.owner.name?:'','-'),3)?:'';

    let $pre_b_damp = array::first(fn::b_pre_type($pe,'DAMP'));
    let $pre_b_mesh = array::first(fn::b_pre_type($pe,'MESH'));
    let $next_b_damp = array::first(fn::b_next_type($pe,'DAMP'));
    let $next_b_mesh = array::first(fn::b_next_type($pe,'MESH'));

    return
    select if fn::hvac_shape($pe) == 'RECT' && array::first($pre_desp)[0] == 614823  && array::first($next_desp)[0] == 614823 {
        $flan_length + ($flan_width * 8) / 1000
    }
    else if fn::hvac_shape($pe) == 'CIRC' && array::first($pre_desp)[0] == 614823  {
        $flan_length
    }

    else if fn::hvac_shape($pe) == 'RECT' && array::first($pre_desp)[0] == 614823  && array::first($next_desp)[0] != 614823 {
        $flan_length + ($flan_width * 8) / 1000 / 2
    }
    else if fn::hvac_shape($pe) == 'CIRC' && array::first($pre_desp)[0] == 614823  {
        $flan_length / 2
    }
    // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND ('[Next_Stype]' EQ 'FJFD')
    else if fn::hvac_shape($pe) == 'RECT' && !$pre_b_damp && !$pre_b_mesh && array::first($next_desp)[0] == 614823 {
        (($flan_length + $flan_width / 1000) * 8) / 4
    } else if fn::hvac_shape($pe) == 'CIRC' && !$pre_b_damp && !$pre_b_mesh && array::first($next_desp)[0] == 614823 {
             $flan_length / 4
    }

    // ((('[Prev_Type]' EQ 'DAMP') AND ('[Prev_Stype]' NE 'FJFD')) OR ('[Prev_Type]' EQ 'MESH')) AND ('[Next_Stype]' EQ 'FJFD')
    else if fn::hvac_shape($pe) == 'RECT'
    && (($pre_b_damp && array::first($pre_desp)[0] != 614823) || pre_b_mesh)
    &&  array::first($next_desp)[0] != 614823 {
           (($flan_length + $flan_width / 1000) * 8) / 2
    } else if fn::hvac_shape($pe) == 'CIRC'
    && (($pre_b_damp && array::first($pre_desp)[0] != 614823) || pre_b_mesh)
    &&  array::first($next_desp)[0] != 614823 {
            $flan_length / 2
    }

    // ((('[Prev_Type]' EQ 'DAMP') AND ('[Prev_Stype]' NE 'FJFD')) OR ('[Prev_Type]' EQ 'MESH')) AND ((('[Next_Type]' EQ 'DAMP') AND ('[Next_Stype]' NE 'FJFD')) OR ('[Next_Type]' EQ 'MESH'))
    else if fn::hvac_shape($pe) == 'RECT'
    && (($pre_b_damp && array::first($pre_desp)[0] != 614823) || $pre_b_mesh)
    && (($next_b_damp && array::first($next_desp)[0] != 614823) || $next_b_mesh) {
               (($flan_length + $flan_width / 1000) * 8)
    }
    else if fn::hvac_shape($pe) == 'CIRC' &&
    (($pre_b_damp && array::first($pre_desp)[0] != 614823) || $pre_b_mesh)
    && (($next_b_damp && array::first($next_desp)[0] != 614823) || $next_b_mesh) {
                $flan_length / 2
    }

    // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND ((('[Next_Type]' EQ 'DAMP') AND ('[Next_Stype]' NE 'FJFD')) OR ('[Next_Type]' EQ 'MESH'))
   else if fn::hvac_shape($pe) == 'RECT' && (!$pre_b_damp && $pre_b_mesh)
   && (($next_b_damp && array::first($next_desp)[0] != 614823) || $next_b_mesh) {
              (($flan_length + $flan_width / 1000) * 8) * 0.75
   }
   else if fn::hvac_shape($pe) == 'CIRC' && (!$pre_b_damp && $pre_b_mesh)
   && (($next_b_damp && array::first($next_desp)[0] != 614823) || $next_b_mesh) {
              $flan_length * 0.75
   }

   // (('[Prev_Type]' NE 'DAMP') AND ('[Prev_Type]' NE 'MESH')) AND (('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH'))
   else if fn::hvac_shape($pe) == 'RECT' && !$pre_b_damp && $pre_b_mesh
   && !$next_b_damp && $next_b_mesh {
              (($flan_length + $flan_width / 1000) * 8) / 2
   } else if fn::hvac_shape($pe) == 'CIRC' && !$pre_b_damp && $pre_b_mesh
   && !$next_b_damp && $next_b_mesh {
              $flan_length / 2
   }
   else {
        ''
    } as size from $pe
};

remove function fn::flex_gask_length_next;
DEFINE FUNCTION fn::flex_gask_length_next($pe:record) {
    let $pre_desp = fn::prev_pe($pe).refno.DESP?:[0.0];
    let $next_desp =  fn::next_pe($pe).refno.DESP?:[0.0];
    let $bran_4 = array::at(string::split($pe.owner.name?:'','-'),3);

    let $pre_b_damp = fn::b_pre_type($pe,'DAMP');
    let $pre_b_mesh = fn::b_pre_type($pe,'MESH');
    let $next_b_damp = fn::b_next_type($pe,'DAMP');
    let $next_b_mesh = fn::b_next_type($pe,'MESH');

    return
    // ('[Prev_Stype]' EQ 'FJFD') AND (('[Next_Type]' NE 'DAMP') AND ('[Next_Type]' NE 'MESH'))
    select if fn::hvac_shape($pe) == 'RECT' && array::first($pre_desp)[0] == 614823 && !$next_b_damp && !$next_b_mesh {
        (($flan_length + $flan_width / 1000) * 8) / 4
    } else if fn::hvac_shape($pe) == 'CIRC' && array::first($pre_desp)[0] == 614823 && !$next_b_damp && !$next_b_mesh {
        $flan_length / 4
    }

    // ('[Prev_Stype]' EQ 'FJFD') AND ((('[Next_Type]' EQ 'DAMP') AND ([Next_Stype]' NE 'FJFD')) OR ('[Next_Type]' EQ 'MESH'))
    else if fn::hvac_shape($pe) == 'RECT' && array::first($pre_desp)[0] == 614823
    && (($next_b_damp && array::first($next_desp)[0] == 614823) || $next_b_mesh) {
        (($flan_length + $flan_width / 1000) * 8) / 2
    } else if fn::hvac_shape($pe) == 'CIRC' && array::first($pre_desp)[0] == 614823
        && (($next_b_damp && array::first($next_desp)[0] == 614823) || $next_b_mesh) {
        $flan_length / 2
    }

    // ('[Prev_Stype]' NE 'FJFD') AND ('[Next_Stype]' EQ 'FJFD')
    else if fn::hvac_shape($pe) == 'RECT' && array::first($pre_desp)[0] != 614823
    && array::first($next_desp)[0] == 614823 {
        (($flan_length + $flan_width / 1000) * 8) / 2
    } else if fn::hvac_shape($pe) == 'CIRC' && array::first($pre_desp)[0] != 614823
    && array::first($next_desp)[0] == 614823 {
        $flan_length / 2
    }
    else {
        ''
    } as size from $pe
};

remove function fn::flex_gask_size;
DEFINE FUNCTION fn::flex_gask_size($pe:record) {
    let $a = <string> array::first(fn::tape_gask_size_pre($pe)).size ?:'';
    let $b = <string>array::first(fn::tape_gask_size_next($pe)).size ?:'';
    return string::concat($a, ',', $b);
};


// return fn::flex_gask_size(pe:24381_57347)