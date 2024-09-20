// 法兰长度
remove function fn::flex_flan_length;
DEFINE FUNCTION fn::flex_flan_length($pe:record) {
    return
    select if fn::hvac_shape($pe) == 'RECT' {
        ((refno.DESP[1]?:0.0 + refno.DESP[2]?:0.0 + refno.DESP[20]?:0.0 * 2 ) * 4 ) / 1000
    } else if fn::hvac_shape($pe) == 'CIRC' {
       ((refno.DESP[1]?:0.0 + refno.DESP[20]?:0.0 * 2 ) * 6.28 ) / 1000
    } else {
        0
    } as length from $pe
};
// return fn::flex_flan_length(pe:24381_88902);