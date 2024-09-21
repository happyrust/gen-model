// 加强筋重量
// $length 加强筋长度
remove function fn::hvac_weight;
DEFINE FUNCTION fn::hvac_weight($pe:record,$size:string,$pressure:string,$length:float,$width:float,$height:float) {
    return
    select value if $size =='RECT' && $pressure == 'L' && math::max([$width,$height]) <= 2000 { $length * 2.422 }
    else if $size =='RECT' && $pressure == 'L' &&
    math::max([$width,$height]) > 2000  { $length * 3.77 }

    else if $size =='RECT' && $pressure == 'M' { $length * 2.422 }
    else if $size =='RECT' && $pressure == 'M' { $length * 3.77 }

    else if $size =='RECT' && $pressure == 'H' && math::max([$width,$height]) <= 1800  { $length * 2.422  }
    else if $size =='RECT' && $pressure == 'H' && math::max([$width,$height]) > 1800 { $length * 3.77 }

    else { 0 } as size from $pe
};