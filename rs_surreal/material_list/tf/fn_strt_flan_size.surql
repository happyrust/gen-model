// 法兰规格
remove function fn::strt_flan_size;
DEFINE FUNCTION fn::strt_flan_size($pe:record) {
    let $desp = $pe.refno.DESP;
    let $fl_fls = $desp[67];
    let $fl_type = if $desp[57] == 531717 { 'FJ' } else if $desp[57] == 531501 { 'FB' } else { '' };
    //return [array::first($desp),$pre_fls,$fl_type];
    return select size from tf_strt_flan_size where fl_type = $fl_type and fl_size = $fl_fls;
};

remove function fn::strt_flan_size_2;
DEFINE FUNCTION fn::strt_flan_size_2($pe:record) {
    let $desp = $pe.refno.DESP;
    let $fl_fls = $desp[68];
    let $fl_type = if $desp[58] == 531717 { 'FJ' } else if $desp[58] == 531501 { 'FB' } else { '' };
    return select size from tf_strt_flan_size where fl_type = $fl_type and fl_size = $fl_fls;
};

// return fn::strt_flan_size_2(pe:24381_57277);