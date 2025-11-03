// 垫片类型
remove function fn::strt_gask_size_pre;
DEFINE FUNCTION fn::strt_gask_size_pre($pe:record) {
    let $pre_desp = fn::prev_pe($pe).refno.DESP?:[0.0];
    let $next_desp = fn::next_pe($pe).refno.DESP?:[0.0];
    let $bran_4 = array::at(string::split($pe.owner.name,'-'),3)?:'';
    return

    select if array::first($pre_desp)[0] == 614823 && fn::b_next_type($pe,'DAMP') {
        'D型'
    }
    else if array::first($pre_desp)[0] != 614823 && ($bran_4 == '1' || $bran_4 == '2') {
        'A型'
    }

    else if array::first($pre_desp)[0] != 614823 && $bran_4 == '1' {
        'B型'
    } else if array::first($pre_desp)[0] == 614823 && array::first($next_desp)[0] != 614823 &&  $bran_b_6 {
        'D型'
    }

    else if array::first($pre_desp)[0] != 614823 && array::first($next_desp)[0] == 614823 &&  ($bran_4 == '1'|| $bran_4 == '2'||$bran_4 == '4') {
        'A型'
    } else if array::first($pre_desp)[0] != 614823 && array::first($next_desp)[0] == 614823 &&  ($bran_4 == '3'|| $bran_4 == '5') {
        'B型'
    } else if array::first($pre_desp)[0] != 614823 && array::first($next_desp)[0] == 614823 &&  $bran_4 == '6' {
        'D型'
    }

    else if array::first($pre_desp)[0] != 614823 && array::first($next_desp)[0] != 614823 &&  ($bran_4 == '1'|| $bran_4 == '2'||$bran_4 == '4') {
        'A型'
    } else if array::first($pre_desp)[0] != 614823 && array::first($next_desp)[0] != 614823 &&  ($bran_4 == '3'|| $bran_4 == '5') {
        'B型'
    } else if array::first($pre_desp)[0] != 614823 && array::first($next_desp)[0] != 614823 &&  $bran_4 == '6' {
        'D型'
    }

    else {
        ''
    } as size from $pe
};

//return fn::strt_gask_size(pe:24381_58570,0)