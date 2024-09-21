// 法兰长度
remove function fn::trns_flan_size;
DEFINE FUNCTION fn::trns_flan_size($pe:record) {
    let $height2 = string::e3d_dehash($pe.refno.DESP[4]);
    let $width = $pe.refno.DESP[1]?:0.0;
    let $width2 = $pe.refno.DESP[3]?:0.0;
    let $height = $pe.refno.DESP[2]?:0.0;
    let $height_str = string::e3d_dehash($height);
    let $fl_width = $pe.refno.DESP[20]?:0.0;
    let $fl_width2 = $pe.refno.DESP[23]?:0.0;

    return if $height2 == 'DIAM' {
        array::join([<string>(($width + $height + $fl_width * 2 ) / 1000),<string>((($width2 + $fl_width2 * 2 ) * 3.14) / 1000)],';')
    } else if $height_str == 'DIAM' {
        array::join([<string>((($width + $fl_width * 2 ) * 3.14) / 1000),<string>((($width2 + $height2 + $fl_width2 * 2 ) * 2) / 1000)],';')
    }
}