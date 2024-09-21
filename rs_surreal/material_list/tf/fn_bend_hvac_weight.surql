//风管重量
remove function fn::bend_hvac_weight;
DEFINE FUNCTION fn::bend_hvac_weight($pe:record) {
    let $bran_4 = array::at(string::split($pe.owner.name,'-')?:[],3)?:'';
    let $angle = $pe.refno.DESP[7]?:0.0;
    let $height = $pe.refno.DESP[2]?:0.0;
    let $hvac_area = array::first(fn::bend_hvac_area($pe).size);
    let $hvac_thick = array::first(fn::bend_hvac_thick($pe).size);
    //return 0;
    return
    select if $angle > 3 && $height > 50 && $bran_4 != '6' {
        $hvac_area * $hvac_thick * 7.85
    } else {
        0
    }
    as size from $pe
};


// return fn::bend_hvac_weight(pe:24381_58571)
