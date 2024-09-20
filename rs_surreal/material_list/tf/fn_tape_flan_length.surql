// 法兰长度
remove function fn::tape_flan_length;
DEFINE FUNCTION fn::tape_flan_length($pe:record) {
    let $desp1 = $pe.refno.DESP[1]?:0.0;
    let $desp2 = $pe.refno.DESP[2]?:0.0;
    let $desp3 = $pe.refno.DESP[3]?:0.0;
    let $desp20 = $pe.refno.DESP[20]?:0.0;
    let $desp23 = $pe.refno.DESP[23]?:0.0;

    let $length =  if fn::hvac_shape($pe) == 'RECT' {
        string::concat(<string>math::fixed((($desp1 + $desp2 * 2 ) * 2 ) / 1000,2),',',<string>math::fixed((($desp3 + $desp23 * 2 ) * 2 ) / 1000,2))
    } else if fn::hvac_shape($pe) == 'CIRC' {
        string::concat(<string>math::fixed((($desp1 + $desp20 * 2 ) * 3.14 ) / 1000,2),',',<string>math::fixed((($desp3 + $desp23 * 2 ) * 3.14 ) / 1000,2))
    } else {
        '0,0'
    };

    return $length;
};
//return fn::tape_flan_length(pe:24381_76894);