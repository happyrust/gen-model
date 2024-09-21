// 螺杆长度
remove function fn::bend_bolt_length_2;
DEFINE FUNCTION fn::bend_bolt_length_2($pe:record) {
    let $bran_4 = array::at(string::split($pe.owner.name,'-')?:[],3)?:'';
    let $hvac_shape = fn::hvac_shape($pe);

    let $angle = $pe.refno.DESP[7];
    let $height = $pe.refno.DESP[2];
    let $width = $pe.refno.DESP[1];

    return
    // ([Angle] GT 60) AND ([Width] LT 900) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    select if $hvac_shape == 'RECT' && $angle > 60 && $width >= 900 && $width < 1400 && $height > 500 && $bran_4 != '6' {
        $height + 50
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
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 900 && $width < 1400 && $height > 500 && $height <= 1000 && $bran_4 != '6' {
        <string>($height + 50)
    }
    // ([Angle] GT 60) AND ([Width] GE 900) AND ([Width] LT 1400) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 900 && $width < 1400 && $height > 1000 && $height < 1500 && !$bran_4 != '6' {
        <string>((4 * $height) / 3 + 50)
    }
    // ([Angle] GT 60) AND ([Width] GE 900) AND ([Width] LT 1400) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 900 && $width < 1400 && $height > 1500 && !$bran_4 != '6' {
        <string>((6 * $height) / 4 + 50)
    }
    // ([Angle] GT 60) AND ([Width] GE 1400) AND ([Width] LT 2000) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 1400 && $width < 2000 && $height > 500 && $height <= 1000 && !$bran_4 != '6' {
        <string>((4 * $height) / 2 + 50)
    }
    // ([Angle] GT 60) AND ([Width] GE 1400) AND ([Width] LT 2000) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 1400 && $width < 2000 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        <string>((8 * $height) / 3 + 50)
    }
    // ([Angle] GT 60) AND ([Width] GE 2000) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 2000 && $height > 500 && $height <= 1000 && !$bran_4 != '6' {
        <string>((6 * $height) / 2 + 50)
    }
    // ([Angle] GT 60) AND ([Width] GE 2000) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 2000 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        <string>((12 * $height) / 3 + 50)
    }
    // ([Angle] GT 60) AND ([Width] GE 2000) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 60 && $width >= 2000 && $height > 1500 && !$bran_4 != '6' {
        <string>((18 * $height) / 4 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 600) AND ([Width] LT 900) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 600 && $width < 900 && $height > 500 && $height <= 1000 && !$bran_4 != '6' {
        <string>((2 * $height) / 2 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 600) AND ([Width] LT 900) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 600 && $width < 900 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        <string>((2 * 2 * $height) / 3 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 600) AND ([Width] LT 900) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 600 && $width < 900 && $height > 1500 && !$bran_4 != '6' {
        <string>((2 * 3 * $height) / 4 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 900) AND ([Width] LT 1200) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 900 && $width < 1200 && $height > 500 && $height <= 1000 && !$bran_4 != '6' {
        <string>((4 * $height) / 2 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 900) AND ([Width] LT 1200) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 900 && $width < 1200 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        <string>((4 * 2 * $height) / 3 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 900) AND ([Width] LT 1200) AND ([Height] GE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 900 && $width < 1200 && $height >= 1500 && !$bran_4 != '6' {
        <string>((4 * 2 * $height) / 4 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1200) AND ([Width] LT 1500) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1200 && $width < 1500 && $height >= 500 && $height <= 1000 && !$bran_4 != '6' {
        <string>(( 6 * $height) / 2 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1200) AND ([Width] LT 1500) AND ([Height] GT 1000) AND ([Height] LE 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1200 && $width < 1500 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        <string>(( 6 * 2 * $height) / 3 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1200) AND ([Width] LT 1500) AND ([Height] GT 1500) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1200 && $width < 1500 && $height > 1500 && !$bran_4 != '6' {
        <string>(( 6 * 3 * $height) / 4 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1500) AND ([Height] GT 500) AND ([Height] LE 1000) AND (Part('[Branch]',4,'-') NE '6')
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1500 && $height > 500 && $height <= 1000 && !$bran_4 != '6' {
        <string>(( 8 * $height) / 2 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1500) AND ([Height] GT 1000) AND ([Height] LE 1500)
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1500 && $height > 1000 && $height <= 1500 && !$bran_4 != '6' {
        <string>(( 8 * 2 * $height) / 3 + 50)
    }
    // ([Angle] GT 30) AND ([Angle] LE 60) AND ([Width] GE 1500) AND ([Height] GT 1500)
    else if $hvac_shape == 'RECT' && $angle > 30 && $angle <= 60 && $width >= 1500 && $height > 1500 && !$bran_4 != '6' {
        <string>(( 8 * 3 * $height) / 4 + 50)
    }
    else {
        '0'
    } as count from $pe;
};

// return fn::bend_bolt_count(pe:24381_58571)