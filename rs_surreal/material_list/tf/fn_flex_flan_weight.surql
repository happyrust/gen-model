// 法兰重量
remove function fn::flex_flan_weight;
DEFINE FUNCTION fn::flex_flan_weight($pe:record) {
    let $desp = $pe.refno.DESP;
    let $flan_type = if array::first($desp)[57] == 531717 { 'FJ' } else if array::first($desp)[57] == 531501 { 'FB' } else { '' };
    let $flan_length = array::first(fn::flex_flan_length($pe)).length?:0.0;

    // return [$flan_type,array::first($desp)[67],$flan_length];
    return
    select if ($flan_type == 'FJ' && array::first($desp)[67] == 25) {
        <float>$flan_length * 1.124
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 303) {
        <float>$flan_length * 1.373
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 30) {
        <float>$flan_length * 1.786
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 35) {
        <float>$flan_length * 1.656
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 40) {
        <float>$flan_length * 2.422
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 45) {
        <float>$flan_length * 2.736
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 50) {
        <float>$flan_length * 3.77
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 60) {
        <float>$flan_length * 5.369
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 608) {
        <float>$flan_length * 7.081
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 6550) {
        <float>$flan_length * 5.134
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 65) {
        <float>$flan_length * 5.3694
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 63) {
        <float>$flan_length * 5.721
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 70) {
        <float>$flan_length * 7.398
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 7550) {
        <float>$flan_length * 5.699
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 75) {
        <float>$flan_length * 7.976
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 8060) {
        <float>$flan_length * 6.311
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 80) {
        <float>$flan_length * 9.658
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 8010) {
        <float>$flan_length * 11.874
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 90) {
        <float>$flan_length * 5.661
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 9056) {
        <float>$flan_length * 5.661
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 10065) {
        <float>$flan_length * 9.8596
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 10080) {
        <float>$flan_length * 10.946
    } else if ($flan_type == 'FJ' && array::first($desp)[67] == 100) {
        <float>$flan_length * 12.276
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 253) {
        <float>$flan_length * 0.59
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 254) {
        <float>$flan_length * 0.78
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 304) {
        <float>$flan_length * 0.94
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 305) {
        <float>$flan_length * 1.18
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 354) {
        <float>$flan_length * 1.10
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 404) {
        <float>$flan_length * 1.26
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 405) {
        <float>$flan_length * 1.57
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 505) {
        <float>$flan_length * 1.96
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 606) {
        <float>$flan_length * 2.826
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 608) {
        <float>$flan_length * 3.77
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 6010) {
        <float>$flan_length * 4.71
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 808) {
        <float>$flan_length * 5.02
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 8010) {
        <float>$flan_length * 6.28
    } else if ($flan_type == 'FB' && array::first($desp)[67] == 8012) {
        <float>$flan_length * 7.54
    } else {
        <float>$flan_length * 7.54
    } as weight from $pe
};

// return fn::flex_flan_weight(pe:24381_57347);
