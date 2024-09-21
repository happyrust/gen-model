// 加强筋规格
remove function fn::hvac_size;
DEFINE FUNCTION fn::hvac_size($pe:record,$size:string,$pressure:string) {
    let $desp1 = $pe.refno.DESP[1]?:0.0;
    let $desp2 = $pe.refno.DESP[2]?:0.0;
    return
    select if $size =='RECT' && $pressure == 'L' && math::max([$desp1,$desp2]) <= 800 { '-' } else if
    $size =='RECT' && $pressure == 'L' &&
    math::max([$desp1,$desp2]) > 800 && math::max([$desp1,$desp2]) <= 2000 && array::at(string::split(owner.name,'-'),3) != '6' { 'L40x4'} else if $size =='RECT' && $pressure == 'L' &&
    math::max([$desp1,$desp2]) > 2000 && array::at(string::split(owner.name,'-'),3) != '6' { 'L50x5' }

    else if $size =='RECT' && $pressure == 'M' && math::max([$desp1,$desp2]) <= 800 { '-' }
    else if $size =='RECT' && $pressure == 'M' && math::max([$desp1,$desp2]) > 800 && math::max([$desp1,$desp2]) <= 2000 && array::at(string::split(owner.name,'-'),3) != '6' { 'L40x4'}
    else if $size =='RECT' && $pressure == 'M' &&
    math::max([$desp1,$desp2]) > 2000 { 'L50x5' }

    else if $size =='RECT' && $pressure == 'H' && math::max([$desp1,$desp2]) <= 600 { '-' }
    else if $size =='RECT' && $pressure == 'H' && math::max([$desp1,$desp2]) > 600 && math::max([$desp1,$desp2]) <= 1800 && array::at(string::split(owner.name,'-'),3) != '6' { 'L40x4'}
    else if $size =='RECT' && $pressure == 'H' &&
    math::max([$desp1,$desp2]) > 1800 { 'L50x5' }

    else { '-' } as size from $pe
};