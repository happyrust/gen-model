//风管面积
remove function fn::bend_hvac_area;
DEFINE FUNCTION fn::bend_hvac_area($pe:record) {
    let $bran_4 = array::at(string::split($pe.owner.name,'-')?:[],3)?:'';
    let $angle = $pe.refno.DESP[7];
    let $height = $pe.refno.DESP[2];
    let $width = $pe.refno.DESP[1];
    return
    select if $angle > 30 && $height > 500 && $height <= 1000 && $bran_4 != '6' {
        ($width * ( $angle *3.14 / 180 * ($height / 2 + 100))) / 1000000
    } else if $angle > 30 && $height > 500 && $height > 1000 && $height <= 1500  && $bran_4 != '6' {
        ($width * ( $angle * 3.14 / 180 * ( $height + 200))) / 1000000
    } else if $angle > 30 && $height > 500 && $height > 1500  && $bran_4 != '6' {
        ($width * ( $angle * 3.14 / 180 * ($height * 1.5 + 300 ))) / 1000000
    } else {
        0
    }
    as size from $pe
};


//return fn::bend_hvac_area(pe:24381_58571)