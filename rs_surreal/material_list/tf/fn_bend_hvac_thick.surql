//风管壁厚
remove function fn::bend_hvac_thick;
DEFINE FUNCTION fn::bend_hvac_thick($pe:record) {
    let $pre_desp = fn::prev_pe($pe).refno.DESP?:[0.0];
    let $bran_4 = array::at(string::split($pe.owner.name,'-')?:[],3)?:'';
    let $angle = $pe.refno.DESP[7];
    let $height = $pe.refno.DESP[2];
    return
    select if $angle > 30 && $height > 500 && $bran_4 != '6' {
        $pe.refno.DESP[32]
    } else {
        0
    } as size from $pe
};


// return fn::bend_hvac_thick(pe:24381_58571)