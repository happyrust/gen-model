// 圆管与方形风管
remove function fn::hvac_shape;
DEFINE FUNCTION fn::hvac_shape($pe:record) {
    return
    if $pe.refno.DESP[29] == 927441 && $pe.refno.DESP[39] == 927441 {'RECT'} else if refno.DESP[29] == 603858 && refno.DESP[39] == 603858 { 'CIRC' } else { '-' }
};