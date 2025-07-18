//适用于STRT
DEFINE FUNCTION fn::hvac_bolt_qty($pe: record) {
       let $mat_id = record::id(fn::hvac_mat_id($pe)?:'');
       return if ($mat_id?:0) == 6 {
           return 0.0;
       } else {
       let $is_circle = (fn::hvac_shape($pe)?:'') == 'CIRC';
       let $pd = fn::pre_is_type($pe, 'DAMP')?:false;
       let $nd = fn::next_is_type($pe, 'DAMP')?:false;
       let $pm = fn::pre_is_type($pe, 'MESH')?:false;
       let $nm = fn::next_is_type($pe, 'MESH')?:false;
       let $pf = fn::prev_stype_is_fjfd($pe)?:false;
       let $nf = fn::next_stype_is_fjfd($pe)?:false;

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
           return math::round( (( (fn::hvac_fl_len($pe)?:0) + 75.0 ) / 150.0) * ($ratio?:0) );
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
            return (math::round(((fn::hvac_width($pe)?:0) + (fn::hvac_fl_width($pe)?:0)) / 150.0 + 1.5 ) +
                                                                math::round(((fn::hvac_height($pe)?:0) + (fn::hvac_fl_width($pe)?:0)) / 150.0 + 1.5 )) * ($ratio?:0);
       };
       }
};



//适用于TAPE
DEFINE FUNCTION fn::hvac_bolt_qtys_1($pe: record) {
       let $mat_id = record::id(fn::hvac_mat_id($pe)?:'');
       return if ($mat_id?:0) == 6 {
           return 0.0;
       } else {
       let $is_circle = (fn::hvac_shape($pe)?:'') == 'CIRC';
       let $pd = fn::pre_is_type($pe, 'DAMP')?:false;
       let $nd = fn::next_is_type($pe, 'DAMP')?:false;
       let $pm = fn::pre_is_type($pe, 'MESH')?:false;
       let $nm = fn::next_is_type($pe, 'MESH')?:false;

       return if $is_circle {
            let $d1 = if ( $pd || $pm ) {
                return math::round(((fn::hvac_fl_len($pe)?:0) + 75.0 ) / 150.0);
             } else if (!$pd) && (!$pm) {
                return math::round(((fn::hvac_fl_len($pe)?:0) + 75.0 ) / 150.0 / 2.0);
             };

             let $d2 = if ( $nd || $nm ) {
                 return math::round(((fn::hvac_fl_len($pe)?:0) + 75.0 ) / 150.0);
              } else if (!$nd) && (!$nm) {
                 return math::round(((fn::hvac_fl_len($pe)?:0) + 75.0 ) / 150.0 / 2.0);
              };
           return [($d1?:0), ($d2?:0)];
       } else {
            let $d1 = if $pd || $pm {
                return ( math::round( ((fn::hvac_width($pe)?:0) + ((fn::hvac_fl_width($pe)?:0) * 2.0) + 75.0) / 150.0 )
                    + math::round( ((fn::hvac_height($pe)?:0) + ((fn::hvac_fl_width($pe)?:0) * 2.0) + 75.0) / 150.0 ) - 2) * 2;
             } else if (!$pd) && (!$pm) {
                return ( math::round( ((fn::hvac_width($pe)?:0) + ((fn::hvac_fl_width($pe)?:0) * 2.0) + 75.0) / 150.0 )
                                    + math::round( ((fn::hvac_height($pe)?:0) + ((fn::hvac_fl_width($pe)?:0) * 2.0) + 75.0) / 150.0 ) - 2);
             };

             let $d2 = if $nd || $nm {
                return math::round( ( ((fn::hvac_width($pe)?:0) + ((fn::hvac_fl_width($pe)?:0) * 2.0) + 75.0) / 150.0 ) +
                                    ( ((fn::hvac_height($pe)?:0) + ((fn::hvac_fl_width($pe)?:0) * 2.0) + 75.0) / 150.0 ) - 2) * 2;
              } else if (!$nd) && (!$nm) {
                 return ( math::round( ((fn::hvac_width2($pe)?:0) + ((fn::hvac_fl_width2($pe)?:0) * 2.0) + 75.0) / 150.0 )
                                     + math::round( ((fn::hvac_height2($pe)?:0) + ((fn::hvac_fl_width2($pe)?:0) * 2.0) + 75.0) / 150.0 )  - 2);
              };
           return [($d1?:0), ($d2?:0)];
       };
       };
};


DEFINE FUNCTION fn::hvac_washer_bolt_qtys_1($pe: record) {
    let $b = fn::hvac_bolt_qtys_1($pe)?:[];
    return [($b[0]?:0) * 2, ($b[1]?:0) * 2];
};



DEFINE FUNCTION fn::hvac_bolt_qtys_THRE($pe: record) {
    let $is_circle = (fn::hvac_shape($pe)?:'') == 'CIRC';
    let $pd = fn::pre_is_type($pe, 'DAMP')?:false;
    let $nd = fn::next_is_type($pe, 'DAMP')?:false;
    let $pm = fn::pre_is_type($pe, 'MESH')?:false;
    let $nm = fn::next_is_type($pe, 'MESH')?:false;
    let $cd = fn::cref_is_type($pe, 'DAMP')?:false;
    let $cm = fn::cref_is_type($pe, 'MESH')?:false;

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
    let $a = fn::hvac_fl_len_THRE($pe)?:[];
    return if $is_circle {
        let $d1 = fn::cal_number_formula_2(($a[0])?:0)?:0;
        let $d2 = fn::cal_number_formula_2(($a[1])?:0)?:0;
        let $d3 = fn::cal_number_formula_2(($a[2])?:0)?:0;
        return [ math::round(($d1?:0) * ($r1?:0)), math::round(($d2?:0) * ($r2?:0)), math::round(($d3?:0) * ($r3?:0))];
    } else {
        let $d1 = fn::cal_number_formula_1($pe, 1)?:0;
        let $d2 = fn::cal_number_formula_1($pe, 2)?:0;
        let $d3 = fn::cal_number_formula_1($pe, 3)?:0;
        return [ math::round(($d1?:0) * ($r1?:0)), math::round(($d2?:0) * ($r2?:0)), math::round(($d3?:0) * ($r3?:0))];
    };

};


DEFINE FUNCTION fn::hvac_washer_bolt_qtys_THRE($pe: record) {
    let $b = fn::hvac_bolt_qtys_THRE($pe)?:[];
    return [($b[0]?:0) * 2, ($b[1]?:0) * 2, ($b[2]?:0) * 2];
};


DEFINE FUNCTION fn::hvac_bolt_qtys_TRNS($pe: record) {
    let $pd = fn::pre_is_type($pe, 'DAMP')?:false;
    let $nd = fn::next_is_type($pe, 'DAMP')?:false;
    let $pm = fn::pre_is_type($pe, 'MESH')?:false;
    let $nm = fn::next_is_type($pe, 'MESH')?:false;

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

     let $a = fn::hvac_fl_len_TRNS($pe)?:[];
     return if (fn::hvac_height2_is_DIAM($pe)?:false) {
            let $d1 = fn::cal_number_formula_1($pe)?:0;
            let $d2 = fn::cal_number_formula_2(($a[0])?:0)?:0;
            return [ math::round(($d1?:0) * ($r1?:0)), math::round(($d2?:0) * ($r2?:0))];
        } else {
            let $d1 = fn::cal_number_formula_2(($a[1])?:0)?:0;
            let $d2 = fn::cal_number_formula_1($pe)?:0;
            return [ math::round(($d1?:0) * ($r1?:0)), math::round(($d2?:0) * ($r2?:0))];
        };
};

DEFINE FUNCTION fn::hvac_washer_bolt_qtys_TRNS($pe: record) {
    let $b = fn::hvac_bolt_qtys_TRNS($pe)?:[];
    return [($b[0]?:0) * 2, ($b[1]?:0) * 2];
};

DEFINE FUNCTION fn::hvac_bolt_qtys_OFST($pe: record) {
    let $pd = fn::pre_is_type($pe, 'DAMP')?:false;
    let $nd = fn::next_is_type($pe, 'DAMP')?:false;
    let $pm = fn::pre_is_type($pe, 'MESH')?:false;
    let $nm = fn::next_is_type($pe, 'MESH')?:false;
    let $is_circle = (fn::hvac_shape($pe)?:'') == 'CIRC';
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
        let $a = fn::hvac_fl_len_OFST($pe)?:0;
        let $d1 = fn::cal_number_formula_2($a)?:0;
        return math::round(($d1?:0) * ($ratio?:0));

    } else {
        let $d1 = fn::cal_number_formula_1($pe)?:0;
        return math::round(($d1?:0) * ($ratio?:0) / 4.0);  // 相当于 1.0 0.75 0.75 0.5
    };

};