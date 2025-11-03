
//[Stype]	BOOT				real([Length] + [A_Len] + [L_Len])
//[Stype]	TABR				real([Length] + [L_Len])
//[Stype]	ANGB				real([Length] + [L_Len])
//[Stype]	SBRA				real([Length] + [A_Len] + [L_Len])
//[Stype]	FISH				real([Length] + [A_Len] + [L_Len])


//BRCO 的长度
DEFINE FUNCTION fn::hvac_len_BRCO($pe: record) {
     let $s = fn::stype($pe);
     return if $s in ['BOOT', 'SBRA', 'FISH']  {
            return fn::hvac_len($pe) + fn::hvac_len_A($pe) + fn::hvac_len_L($pe);
        } else if $s in [ 'TABR', 'ANGB' ]  {
            return fn::hvac_len($pe) + fn::hvac_len_L($pe);
        };
};