
remove function fn::gy_bran;
define function fn::gy_bran($refnos: array<record>, $filter_noun: string) {
    let $children = return if $filter_noun == '' {
        return select value in from array::flatten($refnos<-pe_owner);
    } else {
        return select value in from array::flatten($refnos<-pe_owner) where in.noun == $filter_noun;
    };
    // return $children;
    return if $filter_noun == 'BEND' {
        return fn::gy_bend($children);
    } else if $filter_noun == 'TUBI' {
        return fn::gy_tubi($children);
    } else {
        return fn::gy_part($children);
    }
};

-- let $refno = [pe:24383_84088];
-- let $bran = fn::gy_bran($refno, 'BEND');
-- return $bran;


remove function fn::gy_single;
define function fn::gy_single($refno: record) {
    let $pes = [$refno];
    let $filter_noun = $refno.noun;
    return if $filter_noun == 'BEND' {
        return fn::gy_bend($pes);
    } else if $filter_noun == 'TUBI' {
        return fn::gy_tubi($pes);
    } else if $filter_noun == 'VALVE' {
        return fn::gy_valve($pes);
    } else {
        return fn::gy_part($pes);
    }
};