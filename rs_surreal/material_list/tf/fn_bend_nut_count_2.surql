// 螺母数量
remove function fn::bend_bolt_count_2;
DEFINE FUNCTION fn::bend_bolt_count_2($pe:record) {
    let $bran_4 = array::at(string::split($pe.owner.name,'-')?:[],3)?:'';
    let $hvac_shape = fn::hvac_shape($pe);

    let $angle = $pe.refno.DESP[7]?:0.0;
    let $height = $pe.refno.DESP[2]?:0.0;
    let $width = $pe.refno.DESP[1]?:0.0;

    return
    // ([Angle] GT 60) AND ([Width] LT 900) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    select if $hvac_shape == 'RECT' && $angle > 60 && $width < 900 && $height > 500 && $height <= 1000 && $bran_4 != '6' {
        '3'
    }
    // ([Angle] GT 60) AND ([Width] LT 900) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width < 900 && $height > 1000 && $height <= 1500 && $bran_4 != '6' {
        '6'
    }
    // ([Angle] GT 60) AND ([Width] LT 900) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width < 900 && $height > 1500 && $bran_4 != '6' {
        '9'
    }
    // ([Angle] GT 60) AND ([Width] GE 900) AND ([Width] LT 1400) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width <= 900 && $width < 1400 && $height > 500 && $height <= 1000 && $bran_4 != '6' {
        '11'
    }
    // ([Angle] GT 60) AND ([Width] GE 900) AND ([Width] LT 1400) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 900 && $width < 1400 && $height > 1000 && $height <= 1500 && $bran_4 != '6' {
        '10'
    }
    // ([Angle] GT 60) AND ([Width] GE 900) AND ([Width] LT 1400) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 900 && $width < 1400 && $height > 1500 && !$bran_4 != '6' {
        '13'
    }
    // ([Angle] GT 60) AND ([Width] GE 1400) AND ([Width] LT 2000) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 1400 && $width < 2000 && $height > 500 && $height <= 1000 && !$bran_4 != '6' {
        '19'
    }
    // ([Angle] GT 60) AND ([Width] GE 1400) AND ([Width] LT 2000) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 1400 && $width < 2000 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        '14'
    }
    // ([Angle] GT 60) AND ([Width] GE 2000) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 2000 && $height > 500 && $height <= 1000 && !$bran_4 != '6' {
        '27'
    }
    // ([Angle] GT 60) AND ([Width] GE 2000) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 2000 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        '18'
    }
    // ([Angle] GT 60) AND ([Width] GE 2000) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 2000 && $height > 1500 && !$bran_4 != '6' {
        '21'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] LT 600) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width < 600 && $height > 500 && $height <= 1000 && !$bran_4 != '6' {
        '3'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] LT 600) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width < 600 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        '6'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] LT 600) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width < 600 && $height > 1500 && !$bran_4 != '6' {
        '6'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 600) AND ([Width] LT 900) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 600 && $width < 900 && $height > 500 && $height <= 1000 && !$bran_4 != '6' {
        '11'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 600) AND ([Width] LT 900) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 600 && $width < 900 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        '10'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 600) AND ([Width] LT 900) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 600 && $width < 900 && $height > 1500 && !$bran_4 != '6' {
        '13'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 900) AND ([Width] LT 1200) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 900 && $width < 1200 && $height > 500 && $height <= 1000 && !$bran_4 != '6' {
        '11'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 900) AND ([Width] LT 1200) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 900 && $width < 1200 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        '14'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 900) AND ([Width] LT 1200) AND ([Height] GE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 900 && $width < 1200 && $height >= 1500 && !$bran_4 != '6' {
        '17'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1200) AND ([Width] LT 1500) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1200 && $width < 1500 && $height >= 500 && $height <= 1000 && !$bran_4 != '6' {
        '27'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1200) AND ([Width] LT 1500) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1200 && $width < 1500 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        '18'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1200) AND ([Width] LT 1500) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1200 && $width < 1500 && $height > 1500 && !$bran_4 != '6' {
        '21'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1500) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1500 && $height > 500 && $height <= 1000 && !$bran_4 != '6' {
        '35'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1500) AND ([Height] GT 1000) AND ([Height] LE 1500)
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1500 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        '22'
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1500) AND ([Height] GT 1500)
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1500 && $height > 1500 && !$bran_4 != '6' {
        '25'
    }
    else {
        '0'
    } as count from $pe;
};

// return fn::bend_bolt_count(pe:24381_58571)