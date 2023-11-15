--通过传递refno，查询CATR属性值
let $a = array::distinct(select value [refno.CATR.refno.*, refno.SPRE.refno.CATR.refno.*, refno.CATR.TABITE.PRTREF.refno.*] from only (type::thing("pe", $refno)));
if $a[0] != none {
    return $a[0];
} else {
    return $a[1];
}