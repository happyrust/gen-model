// 加强筋长度
remove function fn::hvac_length;
DEFINE FUNCTION fn::hvac_length($pe:record,$size:string,$pressure:string,$length:float,$width:float,$height:float) {
    return
    select value if $size =='RECT' && $pressure == 'L' && math::max([$width,$height]) <= 800 { 0 }
    else if
    $size =='RECT' && $pressure == 'L' &&
    math::max([$width,$height]) > 800 && math::max([$width,$height]) <= 2000 && array::at(string::split(owner.name,'-'),3) != '6' { ($width + $height + 82) / 500 }
    else if $size =='RECT' && $pressure == 'L' &&
    math::max([$width,$height]) > 2000 { ($width + $height + 103) * ($length / 800 - 0.5) / 500}

    else if $size =='RECT' && $pressure == 'M' && math::max([$width,$height]) <= 800 { 0 }
    else if $size =='RECT' && $pressure == 'M' && math::max([$width,$height]) > 800 && math::max([$width,$height]) <= 2000 && array::at(string::split(owner.name,'-'),3) != '6' { ($width + $height + 83) / 500 }
    else if $size =='RECT' && $pressure == 'M' &&
    math::max([$width,$height]) > 2000 && array::at(string::split(owner.name,'-'),3) != '6' { ($width+$height+103) * ($length / 800 - 0.5) / 500 }

    else if $size =='RECT' && $pressure == 'H' && math::max([$width,$height]) <= 600 { 0 }
    else if $size =='RECT' && $pressure == 'H' && math::max([$width,$height]) > 600 && math::max([$width,$height]) <= 1800 && array::at(string::split(owner.name,'-'),3) != '6' { ($width + $height + 83) / 500 }
    else if $size =='RECT' && $pressure == 'H' &&
    math::max([$width,$height]) > 1800 { ($width+$height+103) * ($length / 800 - 0.5) / 500 }

    else { 0 } as size from $pe
};