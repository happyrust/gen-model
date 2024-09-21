// 其他材料类型
remove function fn::bend_other_material_count_2;
DEFINE FUNCTION fn::bend_other_material_count_2($pe:record) {
    let $bran_4 = array::at(string::split($pe.owner.name,'-')?:[],3)?:'';
    let $hvac_shape = fn::hvac_shape($pe);

    let $angle = $pe.refno.DESP[7]?:0.0;
    let $height = $pe.refno.DESP[2]?:0.0;
    let $width = $pe.refno.DESP[1]?:0.0;
    return
    select
    // ([Angle] GT 60) AND ([Width] GE 900) AND ([Width] LT 1400) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    if $hvac_shape == 'RECT' && $angle > 60 && $width <= 900 && $width < 1400 && $height > 500 && $height <= 1000 && $bran_4 != '6' {
        <string>((2 * 2 * $height) / 3)
    }
    // ([Angle] GT 60) AND ([Width] GE 900) AND ([Width] LT 1400) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 900 && $width < 1400 && $height > 1500 && !$bran_4 != '6' {
        <string>((2 * 3 * $height) / 4)
    }
    // ([Angle] GT 60) AND ([Width] GE 1400) AND ([Width] LT 2000) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 1400 && $width < 2000 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        <string>((8 * $height) / 3)
    }
    // ([Angle] GT 60) AND ([Width] GE 2000) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 2000 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        <string>((12 * $height) / 3)
    }
    // ([Angle] GT 60) AND ([Width] GE 2000) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 2000 && $height > 1500 && !$bran_4 != '6' {
        <string>( (18 * $height) / 4)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 600) AND ([Width] LT 900) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 600 && $width < 900 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
       <string>((2 * 2 * $height) / 3)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 600) AND ([Width] LT 900) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 600 && $width < 900 && $height > 1500 && !$bran_4 != '6' {
        <string>((2 * 3 * $height) / 4)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 900) AND ([Width] LT 1200) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 900 && $width < 1200 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        <string>((4 * 2 * $height) / 3)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 900) AND ([Width] LT 1200) AND ([Height] GE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 900 && $width < 1200 && $height >= 1500 && !$bran_4 != '6' {
        <string>((4 * 3 * $height) / 4)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1200) AND ([Width] LT 1500) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1200 && $width < 1500 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        <string>((6 * 2 * $height) / 3)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1200) AND ([Width] LT 1500) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1200 && $width < 1500 && $height > 1500 && !$bran_4 != '6' {
        <string>((6 * 3 * $height) / 4)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1500) AND ([Height] GT 1000) AND ([Height] LE 1500)
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1500 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        <string>((8 * 2 * $height) / 3)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1500) AND ([Height] GT 1500)
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1500 && $height > 1500 && !$bran_4 != '6' {
        <string>((8 * 3 * $height) / 4)
    } else {
        '0'
    } as size from $pe;
};


//return fn::bend_hvac_area(pe:24381_58571)