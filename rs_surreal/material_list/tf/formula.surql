
//string((int(([Width] + [FL_Width] * 2 + 75) / 150) + int(([Height] + [FL_Width] * 2 + 75) / 150) - 2) * 3 , 'D0')
DEFINE FUNCTION fn::cal_number_formula_1($pe: record, $n: option<int>)  {

    return math::round( (fn::hvac_width($pe, $n) + (fn::hvac_fl_width($pe, $n) * 2.0) + 75.0) / 150.0 )
                                + math::round( ( fn::hvac_height($pe, $n) + (fn::hvac_fl_width($pe, $n) * 2.0) + 75.0 ) / 150.0 ) - 2;
};

//string(int(([OUT:FL_Len] + 75) / 150) , 'D0')
DEFINE FUNCTION fn::cal_number_formula_2($d: float)  {
    return (( $d + 75.0 ) / 150.0);
};

//[OUT:FL_Len] * 0.75
DEFINE FUNCTION fn::cal_len_formula_1($fl_len: float, $r: float)  {
    return ( $fl_len * $r);
};

DEFINE FUNCTION fn::cal_len_formula_2($fl_len: float, $r: float)  {
    return ( $fl_len + fn::hvac_fl_width($pe) / 1000.0 * 8.0);
};

//string(([Width] + [Height] + [FL_Width] * 2) * 4 / 1000 , 'D2')
DEFINE FUNCTION fn::cal_len_formula_3()  {
    return ( fn::hvac_width($pe) + fn::hvac_height($pe) + fn::hvac_fl_width($pe) * 2.0) * 4.0 / 1000.0;
};

//string(([Width] + [FL_Width] * 2) * 6.28 / 1000 , 'D2')
DEFINE FUNCTION fn::cal_len_formula_4()  {
    return ( fn::hvac_width($pe) + fn::hvac_fl_width($pe) * 2.0) * 6.28 / 1000.0;
};