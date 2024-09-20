// 法兰长度
remove function fn::strt_flan_length;
DEFINE FUNCTION fn::strt_flan_length($pe:record) {
    let $desp = $pe.refno.DESP;
    let $fl_type = if $desp[57] == 531717 { 'FJ' } else if $desp[57] == 531501 { 'FB' } else { '' };
    //return $fl_type;
    let $pre_desp =  fn::prev_pe($pe).refno.DESP?:[0.0];
    let $next_desp = fn::next_pe($pe).refno.DESP?:[0.0];

    let $desp1 = refno.DESP[1];
    let $desp2 = refno.DESP[2];
    let $desp20 = refno.DESP[20];

    return
    select if fn::hvac_shape($pe) == 'RECT' && $fl_type == 'FJ' &&  array::at(string::split(owner.name,'-'),3) != '6' {
        (($desp1 + $desp2 + $desp20 * 2 + 3) * 4 ) / 1000
    } else if fn::hvac_shape($pe) == 'RECT' && $fl_type == 'FB' &&  array::at(string::split(owner.name,'-'),3) != '6' {
        (($desp1 + $desp2 + $desp20 * 2 + 5) * 4 ) / 1000
    } else if fn::hvac_shape($pe) == 'CIRC' && $fl_type == 'FJ' &&  array::at(string::split(owner.name,'-'),3) != '6' {
        (($desp1 + $desp2 * 2 + 5) * 6.28 ) / 1000
    } else if fn::hvac_shape($pe) == 'CIRC' && $fl_type == 'FB' &&  array::at(string::split(owner.name,'-'),3) != '6' {
        (($desp1 + $desp2 * 2 + 5) * 6.28 ) / 1000
    } else if fn::b_pre_type($pe,'DAMP') &&  array::at(string::split(owner.name,'-'),3) == '6' {
        (($desp1 + $desp2 + $pre_desp[20]?:0.0 * 2 + 3) * 2 ) / 1000
    } else if fn::b_next_type($pe,'DAMP') &&  array::at(string::split(owner.name,'-'),3) == '6' {
        (($desp1 + $desp2 + $next_desp[20]?:0.0 * 2 + 3) * 2 ) / 1000
    } else {
        0
    } as length from $pe
};
