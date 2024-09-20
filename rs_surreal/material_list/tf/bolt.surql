//适用于STRT
DEFINE FUNCTION fn::hvac_bolt_qty($pe: record) {
       let $mat_id = record::id(fn::hvac_mat_id($pe));
       return if $mat_id == 6 {
           return 0.0;
       } else {
       let $is_circle = fn::hvac_shape($pe) == 'CIRC';
       let $pd = fn::pre_is_type($pe, 'DAMP');
       let $nd = fn::next_is_type($pe, 'DAMP');
       let $pm = fn::pre_is_type($pe, 'MESH');
       let $nm = fn::next_is_type($pe, 'MESH');
       let $pf = fn::prev_stype_is_fjfd($pe);
       let $nf = fn::next_stype_is_fjfd($pe);

       return if $is_circle {
           let $ratio = if ($pd || $pm) && ($nd ||  $nm ) {
               return 1.0;
             } else if ($pd || $pm) && (!$nd) &&  (!$nm) {
               return 0.75;
             } else if  (!$pd) &&  (!$pm) && ($nd ||  $nm ) {
               return 0.75;
             } else if (!$pd) &&  (!$pm) && (!$nd) &&  (!$nm) {
               return 0.5;
             };
           return math::round( (( fn::hvac_fl_len($pe) + 75.0 ) / 150.0) * $ratio );
       } else {
            let $ratio = if ($pd || $pm) && ($nd ||  $nm ) {
                   return 4;
                 } else if ($pd || $pm) && (!$nd) &&  (!$nm) {
                   return 3;
                 } else if  (!$pd) &&  (!$pm) && ($nd ||  $nm ) {
                   return 3;
                 } else if (!$pd) &&  (!$pm) && (!$nd) &&  (!$nm) {
                  return 2;
                 };
            return (math::round((fn::hvac_width($pe) + fn::hvac_fl_width($pe)) / 150.0 + 1.5 ) +
                                                                math::round((fn::hvac_height($pe) + fn::hvac_fl_width($pe)) / 150.0 + 1.5 )) * $ratio;
       };
       }
};



//适用于TAPE
DEFINE FUNCTION fn::hvac_bolt_qtys_1($pe: record) {
       let $mat_id = record::id(fn::hvac_mat_id($pe));
       return if $mat_id == 6 {
           return 0.0;
       } else {
       let $is_circle = fn::hvac_shape($pe) == 'CIRC';
       let $pd = fn::pre_is_type($pe, 'DAMP');
       let $nd = fn::next_is_type($pe, 'DAMP');
       let $pm = fn::pre_is_type($pe, 'MESH');
       let $nm = fn::next_is_type($pe, 'MESH');

       return if $is_circle {
            let $d1 = if ( $pd || $pm ) {
                return math::round((fn::hvac_fl_len($pe) + 75.0 ) / 150.0);
             } else if (!$pd) && (!$pm) {
                return math::round((fn::hvac_fl_len($pe) + 75.0 ) / 150.0 / 2.0);
             };

             let $d2 = if ( $nd || $nm ) {
                 return math::round((fn::hvac_fl_len($pe) + 75.0 ) / 150.0);
              } else if (!$nd) && (!$nm) {
                 return math::round((fn::hvac_fl_len($pe) + 75.0 ) / 150.0 / 2.0);
              };
           return [$d1, $d2];
       } else {
            let $d1 = if $pd || $pm {
                return ( math::round( (fn::hvac_width($pe) + (fn::hvac_fl_width($pe) * 2.0) + 75.0) / 150.0 )
                    + math::round( (fn::hvac_height($pe) + (fn::hvac_fl_width($pe) * 2.0) + 75.0) / 150.0 ) - 2) * 2;
             } else if (!$pd) && (!$pm) {
                return ( math::round( (fn::hvac_width($pe) + (fn::hvac_fl_width($pe) * 2.0) + 75.0) / 150.0 )
                                    + math::round( (fn::hvac_height($pe) + (fn::hvac_fl_width($pe) * 2.0) + 75.0) / 150.0 ) - 2);
             };

             let $d2 = if $nd || $nm {
                return math::round( ( (fn::hvac_width($pe) + (fn::hvac_fl_width($pe) * 2.0) + 75.0) / 150.0 ) +
                                    ( (fn::hvac_height($pe) + (fn::hvac_fl_width($pe) * 2.0) + 75.0) / 150.0 ) - 2) * 2;
              } else if (!$nd) && (!$nm) {
                 return ( math::round( (fn::hvac_width2($pe) + (fn::hvac_fl_width2($pe) * 2.0) + 75.0) / 150.0 )
                                     + math::round( (fn::hvac_height2($pe) + (fn::hvac_fl_width2($pe) * 2.0) + 75.0) / 150.0 )  - 2);
              };
           return [$d1, $d2];
       };
       };
};


DEFINE FUNCTION fn::hvac_washer_bolt_qtys_1($pe: record) {
    let $b = fn::hvac_bolt_qtys_1($pe);
    return [$b[0] * 2, $b[1] * 2];
};



DEFINE FUNCTION fn::hvac_bolt_qtys_THRE($pe: record) {
    let $is_circle = fn::hvac_shape($pe) == 'CIRC';
    let $pd = fn::pre_is_type($pe, 'DAMP');
    let $nd = fn::next_is_type($pe, 'DAMP');
    let $pm = fn::pre_is_type($pe, 'MESH');
    let $nm = fn::next_is_type($pe, 'MESH');
    let $cd = fn::cref_is_type($pe, 'DAMP');
    let $cm = fn::cref_is_type($pe, 'MESH');

    let $r1 =  if $pd || $pm {
                 return 1.0;
             } else if (!$pd) && (!$pm) {
                return 0.5;
             };
    let $r2 =  if $nd || $nm {
                     return 1.0;
                 } else if (!$nd) && (!$nm) {
                    return 0.5;
                 };
    let $r3 =  if $cd || $cm {
                         return 1.0;
                     } else if (!$cd) && (!$cm) {
                        return 0.5;
                     };
    let $a = fn::hvac_fl_len_THRE($pe);
    return if $is_circle {
        let $d1 = fn::cal_number_formula_2($a[0]);
        let $d2 = fn::cal_number_formula_2($a[1]);
        let $d3 = fn::cal_number_formula_2($a[2]);
        return [ math::round($d1 * $r1), math::round($d2 * $r2), math::round($d3 * $r3)];
    } else {
        let $d1 = fn::cal_number_formula_1($pe, 1);
        let $d2 = fn::cal_number_formula_1($pe, 2);
        let $d3 = fn::cal_number_formula_1($pe, 3);
        return [ math::round($d1 * $r1), math::round($d2 * $r2), math::round($d3 * $r3)];
    };

};


DEFINE FUNCTION fn::hvac_washer_bolt_qtys_THRE($pe: record) {
    let $b = fn::hvac_bolt_qtys_THRE($pe);
    return [$b[0] * 2, $b[1] * 2, $b[2] * 2];
};


DEFINE FUNCTION fn::hvac_bolt_qtys_TRNS($pe: record) {
    let $pd = fn::pre_is_type($pe, 'DAMP');
    let $nd = fn::next_is_type($pe, 'DAMP');
    let $pm = fn::pre_is_type($pe, 'MESH');
    let $nm = fn::next_is_type($pe, 'MESH');

    let $r1 =  if $pd || $pm {
                     return 1.0;
                 } else if (!$pd) && (!$pm) {
                    return 0.5;
                 } else {
                    return 0.0;
                 };
    let $r2 =  if $nd || $nm {
                     return 1.0;
                 } else if (!$nd) && (!$nm) {
                    return 0.5;
                 } else {
                    return 0.0;
                 };

     let $a = fn::hvac_fl_len_TRNS($pe);
     return if fn::hvac_height2_is_DIAM($pe) {
            let $d1 = fn::cal_number_formula_1($pe);
            let $d2 = fn::cal_number_formula_2($a[0]);
            return [ math::round($d1 * $r1), math::round($d2 * $r2)];
        } else {
            let $d1 = fn::cal_number_formula_2($a[1]);
            let $d2 = fn::cal_number_formula_1($pe);
            return [ math::round($d1 * $r1), math::round($d2 * $r2)];
        };
};

DEFINE FUNCTION fn::hvac_washer_bolt_qtys_TRNS($pe: record) {
    let $b = fn::hvac_bolt_qtys_TRNS($pe);
    return [$b[0] * 2, $b[1] * 2];
};

DEFINE FUNCTION fn::hvac_bolt_qtys_OFST($pe: record) {
    let $pd = fn::pre_is_type($pe, 'DAMP');
    let $nd = fn::next_is_type($pe, 'DAMP');
    let $pm = fn::pre_is_type($pe, 'MESH');
    let $nm = fn::next_is_type($pe, 'MESH');
    let $is_circle = fn::hvac_shape($pe) == 'CIRC';
    let $ratio = if ($pd || $pm) && ($nd || $nm) {
                return 4.0;
            } else if ($pd || $pm) && ( (!$nd) && (!$nm) ) {
                return 3.0;
            } else if ((!$pd) && (!$pm)) && ($nd || $nm) {
                return 3.0;
            } else if ((!$pd) && (!$pm)) && ( (!$nd) && (!$nm) ) {
                return 2.0;
            } else {
                return 0.0;
            };

    return if $is_circle {
        let $a = fn::hvac_fl_len_OFST($pe);
        let $d1 = fn::cal_number_formula_2($a);
        return math::round($d1 * $ratio);

    } else {
        let $d1 = fn::cal_number_formula_1($pe);
        return math::round($d1 * $ratio / 4.0);  // 相当于 1.0 0.75 0.75 0.5
    };

};