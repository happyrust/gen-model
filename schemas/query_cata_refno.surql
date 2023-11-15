--通过传递refno，查询CATR
let $a = array::distinct(select value [refno.CATR, refno.SPRE.refno.CATR, refno.CATR.TABITE.PRTREF] from only (type::thing("pe", $refno)));
if $a[0] != none {
    return $a[0];
} else {
    return $a[1];
}