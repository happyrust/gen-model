
DEFINE FUNCTION fn::common_washer_types($pe: record) {
    return [ fn::common_washer_type_single($pe, true) , fn::common_washer_type_single($pe, false) ];
};


//单个的垫圈类型获取
DEFINE FUNCTION fn::common_washer_type_single($pe: record, $prev_or_next: bool) {
    let $is_fjfd = if $prev_or_next {
        return fn::prev_stype_is_fjfd($pe);
    } else {
        return fn::next_stype_is_fjfd($pe);
    };

    let $stype_is_damp = if $prev_or_next {
         return fn::pre_is_type($pe, 'DAMP');
     } else {
         return fn::next_is_type($pe, 'DAMP');
     };

    let $mat_id = record::id(fn::hvac_mat_id($pe));

    return if $is_fjfd && $stype_is_damp {
        return "D型";
    } else if !$is_fjfd {
        return if $mat_id == 1 || $mat_id == 2 || $mat_id == 4  {
            return "A型";
        } else if $mat_id == 3 || $mat_id == 5 {
            return "B型";
        } else {
            return '';
        }
    };
};


DEFINE FUNCTION fn::common_washer_len($pe: record) {
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

    let $ratio = if (($pd && (!$pf)) ||  $pm ) && (($nd && (!$nf)) ||  $nm ) {
       return 1.0;
    } else if ((!$pd) && (!$pm)) && (($nd && (!$nf)) ||  $nm ) {
       return  0.75;
    } else if (($pd) && (!$pf)) || (($pd && (!$nf)) ||  $nm ) {
       return  0.75;
    } else if ((!$pd) && (!$pm)) && ((!$nd) && (!$nm) ) {
       return  0.5;
    } else {
        0
    };

    return  if $is_circle {
               return math::fixed( ((fn::hvac_fl_len($pe)) / 1000.0 ) * 8.0 * $ratio, 2);
           } else {
               return math::fixed(fn::hvac_fl_len($pe) * $ratio, 2);
           };
    };
};


DEFINE FUNCTION fn::washer_type_CAP($pe: record) {
    let $pf = fn::prev_stype_is_fjfd($pe);
    return if $pf {
        return "D型";
    } else {
        let $mat_id = record::id(fn::hvac_mat_id($pe));
        return if $mat_id in [1, 2, 4] {
            return "B型";
        } else {
            return "A型";
        };
    };
};

DEFINE FUNCTION fn::washer_len_CAP($pe: record) {
    let $is_circle = fn::hvac_shape($pe) == 'CIRC';
    let $pd = fn::pre_is_type($pe, 'DAMP');
    let $pm = fn::pre_is_type($pe, 'MESH');

    return if $is_circle {
        return if $pd || $pm {
            return math::fixed( (fn::hvac_fl_len($pe) + fn::hvac_fl_width($pe) * 4.0) / 1000.0, 2 );
        } else if (!$pd) && (!$pm) {
            return math::fixed( ((fn::hvac_fl_len($pe) + fn::hvac_fl_width($pe) * 4.0) / 1000.0 ) / 2.0, 2);
        } ;
    } else {
        return if $pd || $pm {
            return math::fixed( ( ( (fn::hvac_width($pe) + (fn::hvac_fl_width($pe) * 2.0) + 75.0) / 150.0 ) +
                                                                    ( (fn::hvac_height($pe) + (fn::hvac_fl_width($pe) * 2.0) + 75.0) / 150.0 ) - 2 ) * 2, 2);
        } else if (!$pd) && (!$pm) {
            return math::fixed( ( ( (fn::hvac_width($pe) + (fn::hvac_fl_width($pe) * 2.0) + 75.0) / 150.0 ) +
                                                                    ( (fn::hvac_height($pe) + (fn::hvac_fl_width($pe) * 2.0) + 75.0) / 150.0 ) - 2 ), 2);
        } ;
    };
};

DEFINE FUNCTION fn::washer_type_single($check: bool, $mat_id: int) {
    return if $check {
        return "D型";
    } else {
        return if $mat_id in [1, 2, 4] {
            return "B型";
        } else {
            return "A型";
        };
    };
};

DEFINE FUNCTION fn::washer_type_THRE($pe: record) {
    let $pf = fn::prev_stype_is_fjfd($pe);
    let $nf = fn::next_stype_is_fjfd($pe);
    let $cf = fn::c_stype_is_fjfd($pe);
    let $mat_id = record::id(fn::hvac_mat_id($pe));
    let $m1 =  fn::washer_type_single($pf, $mat_id);
    let $m2 =  fn::washer_type_single($nf, $mat_id);
    let $m3 =  fn::washer_type_single($cf, $mat_id);

    return [$m1, $m2, $m3];
};


DEFINE FUNCTION fn::washer_len_THRE($pe: record) {
    let $is_circle = fn::hvac_shape($pe) == 'CIRC';
    let $pd = fn::pre_is_type($pe, 'DAMP');
    let $nd = fn::next_is_type($pe, 'DAMP');
    let $pm = fn::pre_is_type($pe, 'MESH');
    let $nm = fn::next_is_type($pe, 'MESH');
    let $cd = fn::cref_is_type($pe, 'DAMP');
    let $cm = fn::cref_is_type($pe, 'MESH');
    let $len = fn::hvac_fl_len_THRE($pe);

    return if $is_circle {
        let $d1 = if $pd || $pm {
            return ( $len[0] + fn::hvac_fl_width($pe, 1) * 4.0 / 1000.0 );
        } else if (!$pd) && (!$pm) {
            return ( $len[0] + fn::hvac_fl_width($pe, 1) * 4.0 / 1000.0 ) / 2.0;
        };
        let $d2 = if $nd || $nm {
                    return ( $len[1] + fn::hvac_fl_width($pe, 2) * 4.0 / 1000.0 );
                } else if (!$nd) && (!$nm) {
                    return ( $len[1] + fn::hvac_fl_width($pe, 2) * 4.0 / 1000.0 ) / 2.0;
                };
        let $d3 = if $cd || $cm {
                            return ( $len[2] + fn::hvac_fl_width($pe, 3) * 4.0 / 1000.0 );
                        } else if (!$nd) && (!$nm) {
                            return ( $len[2] + fn::hvac_fl_width($pe, 3) * 4.0 / 1000.0 ) / 2.0;
                        };
        let $r1 = if $d1 != NONE { $d1 } else { 0 };
        let $r2 = if $d2 != NONE { $d2 } else { 0 };
        let $r3 = if $d3 != NONE { $d3 } else { 0 };
        return [$r1, $r2, $r3];
    } else {
        let $d1 = if $pd || $pm {
            return $len[0];
        } else if (!$pd) && (!$pm) {
            return $len[0] / 2.0;
        };
        let $d2 = if $nd || $nm {
            return $len[1];
        } else if (!$nd) && (!$nm) {
            return $len[1] / 2.0;
        };
        let $d3 = if $cd || $cm {
            return $len[2];
        } else if (!$nd) && (!$nm) {
            return $len[2] / 2.0;
        };
        let $r1 = if $d1 != NONE { $d1 } else { 0 };
        let $r2 = if $d2 != NONE { $d2 } else { 0 };
        let $r3 = if $d3 != NONE { $d3 } else { 0 };
        return [$r1, $r2, $r3];
    };
};


DEFINE FUNCTION fn::washer_type_TRNS($pe: record) {
    let $pf = fn::prev_stype_is_fjfd($pe);
    let $nf = fn::next_stype_is_fjfd($pe);
    let $mat_id = record::id(fn::hvac_mat_id($pe));
    let $m1 =  fn::washer_type_single($pf, $mat_id);
    let $m2 =  fn::washer_type_single($nf, $mat_id);

    return [$m1, $m2];
};

DEFINE FUNCTION fn::washer_len_TRNS($pe: record) {
    let $pd = fn::pre_is_type($pe, 'DAMP');
    let $nd = fn::next_is_type($pe, 'DAMP');
    let $pm = fn::pre_is_type($pe, 'MESH');
    let $nm = fn::next_is_type($pe, 'MESH');
    let $len = fn::hvac_fl_len_TRNS($pe);

    return if fn::hvac_height2_is_DIAM($pe) {
        let $d1 = if $pd || $pm {
                    return ( $len[0] + fn::hvac_fl_width($pe, 1) * 4.0 / 1000.0 );
                } else if (!$pd) && (!$pm) {
                    return ( $len[0] + fn::hvac_fl_width($pe, 1) * 4.0 / 1000.0 ) / 2.0;
                };
         let $d2 = if $nd || $nm {
                            return ( $len[0]  );
                        } else if (!$nd) && (!$nm) {
                            return ( $len[0] ) / 2.0;
                        };
         return [math::fixed($d1, 2), math::fixed($d2, 2)];
    } else if fn::hvac_height_is_DIAM($pe) {
        let $d1 = if $pd || $pm {
                    return ( $len[1] + fn::hvac_fl_width($pe, 2) * 4.0 / 1000.0 );
                } else if (!$pd) && (!$pm) {
                    return ( $len[1] + fn::hvac_fl_width($pe, 2) * 4.0 / 1000.0 ) / 2.0;
                };
         let $d2 = if $nd || $nm {
                            return ( $len[1]  );
                        } else if (!$nd) && (!$nm) {
                            return ( $len[1] ) / 2.0;
                        };
         return [math::fixed($d1, 2), math::fixed($d2, 2)];
    };
};

DEFINE FUNCTION fn::washer_len_BEND($pe: record) {
    let $mat_id = record::id(fn::hvac_mat_id($pe));
    return if $mat_id == 6 {
        return [0.0];
    } else {
    let $pf = fn::prev_stype_is_fjfd($pe);
    let $nf = fn::next_stype_is_fjfd($pe);

    let $pd = fn::pre_is_type($pe, 'DAMP');
    let $nd = fn::next_is_type($pe, 'DAMP');
    let $pm = fn::pre_is_type($pe, 'MESH');
    let $nm = fn::next_is_type($pe, 'MESH');

    let $is_circle = fn::hvac_shape($pe) == 'CIRC';

    let $fl_len = fn::hvac_fl_len($pe);
    let $fl_width = if $pe.refno.DESP[20] != NONE { $pe.refno.DESP[20] } else { 0 };

    //return $is_circle;
    //return [$fl_len,$fl_width,!$pd , !$pm , !$nd , !$nm];
    //return [($fl_len + ( ($fl_width * 8 ) / 1000 ))/ 2];
    let $first = if !$is_circle {
        return if $pf && $nf {
            return ($fl_len + ( ($fl_width * 8 ) / 1000 ));
        } else if $pf && !$nf {
            return ($fl_len + ( ($fl_width * 8 ) / 1000)) / 2 ;
        } else if $pf && !$pd && !$pm {
            return ($fl_len + ( ($fl_width * 8 ) / 1000 ))/ 4;
        } else if (($pd && !$pf) || $pm) && $nf {
            return ($fl_len + ( ($fl_width * 8 ) / 1000 ))/ 2;
        } else if (($pd && !$pf) || $pm) && (($nd && !$nf) || $nm) {
            return ($fl_len + ( ($fl_width * 8 ) / 1000 ));
        } else if ($pd && !$pm) && (($nd && !$nf) || $nm) {
            return ($fl_len + ( ($fl_width * 8 ) / 1000 )) * 0.75;
        } else if (($pd && !$pf) || $pm) && !$nm {
            return ($fl_len + ( ($fl_width * 8 ) / 1000 )) * 0.75;
        } else if !$pd && !$pm && !$nd && !$nm {
            return ($fl_len + ( ($fl_width * 8 ) / 1000 ))/ 2;
        }
    } else {
        return if $pf && $nf {
            return $fl_len;
        } else if $pf && !$nf {
            return $fl_len / 2;
        } else if $pf && !$pd && !$pm {
            return $fl_len / 4;
        } else if (($pd && !$pf) || $pm) && $nf {
            return $fl_len / 2;
        } else if (($pd && !$pf) || $pm) && (($nd && !$nf) || $nm) {
            return $fl_len;
        } else if ($pd && !$pm) && (($nd && !$nf) || $nm) {
            return $fl_len * 0.75;
        } else if (($pd && !$pf) || $pm) && !$nm {
            return $fl_len * 0.75;
        } else if !$pd && !$pm && !$nd && !$nm {
            return $fl_len / 2;
        }
     };
    let $second =  if !$is_circle {
        return if $pf && !$nd && !$nm {
            return ($fl_len + ( ($fl_width * 8 ) / 1000)) / 4;
        } else if $pf && (($nd && $nf) || $nm) {
            return ($fl_len + ( ($fl_width * 8 ) / 1000 ))/ 2;
        } else if !$pf && $nf {
            return ($fl_len + ( ($fl_width * 8 ) / 1000 ))/ 2;
        }
    } else {
        return if $pf && !$nd && !$nm {
            return $fl_len / 4;
        } else if $pf && (($nd && $nf) || $nm) {
            return $fl_len / 2;
        } else if !$pf && $nf {
            return $fl_len / 2;
        }
    };

    let $r1 = if $first != NONE { math::fixed($first,2) } else { 0 };
    let $r2 = if $second != NONE { math::fixed($second,2) } else { 0 };

    return [$r1,$r2];
    };
};
