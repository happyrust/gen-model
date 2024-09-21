
DEFINE FUNCTION fn::hvac_stype($pe: record){
    return $pe.refno.DESP[0];
};

DEFINE FUNCTION fn::hvac_mat_number($pe: record){
    return <int>array::at(string::split(($pe->pe_owner->pe.name)[0], '-'), 3);
};


DEFINE FUNCTION fn::hvac_mat_id($pe: record){
    return type::thing('hvac_mat', <int>array::at(string::split(($pe->pe_owner->pe.name)[0], '-'), 3));
};

//获得风管系统
DEFINE FUNCTION fn::hvac_system($pe: record){
    return array::at(string::split(($pe->pe_owner->pe.name)[0], '-'), 1);
};


DEFINE FUNCTION fn::hvac_number($pe: record){
    let $v = (select value <int>v from type::thing('ATT_UDA', record::id($pe)).udas where u.NAME='/H-SECTCODE')[0];
    return if $v == none {
        return "unset";
    } else {
        return $v;
    };
};

DEFINE FUNCTION fn::hvac_bran_name_splits($pe: record){
    return array::complement((select value string::split((->pe_owner->pe.name)[0], '-') from $pe)[0], ['']);
};

DEFINE FUNCTION fn::hvac_zone_name_splits($pe: record){
    return (select value string::split((->pe_owner->pe->pe_owner->pe->pe_owner->pe.name)[0], '-') from $pe)[0];
};

DEFINE FUNCTION fn::hvac_seg_code($pe: record){
    let $s = fn::hvac_bran_name_splits($pe);
    return string::join('/', string::concat($s[2], $s[5]), fn::hvac_number($pe));
};

DEFINE FUNCTION fn::hvac_sub_code($pe: record){
    let $s = fn::hvac_zone_name_splits($pe);
    return $s[2];
};

DEFINE FUNCTION fn::hvac_shape($pe: record){
    return if $pe.refno.DESP[29] == 927441 {
        return "RECT";
    } else if $pe.refno.DESP[29] == 603858 {
        return "CIRC";
    } else {
        return "unset";
    };
};

DEFINE FUNCTION fn::shape_name($pe: record){
    return if $pe.refno.DESP[29] == 927441 {
        return "方形";
    } else if $pe.refno.DESP[29] == 603858 {
        return "圆形";
    } else {
        return "unset";
    };
};


//获得风管宽度数组
DEFINE FUNCTION fn::hvac_width_format($pe: record, $n: option<int>){
    let $is_circle = fn::hvac_shape($pe) == 'CIRC';
    let $w = fn::hvac_width($pe, $n);
    return if $is_circle {
        return string::concat('φ', $w);
    } else {
        return $w;
    };
};

//获得风管高度数组
DEFINE FUNCTION fn::hvac_height_format($pe: record, $n: option<int>){
    let $is_circle = fn::hvac_shape($pe) == 'CIRC';
    let $h = fn::hvac_height($pe, $n);
    return if $is_circle {
        return '-';
    } else {
        return $h;
    };
};

DEFINE FUNCTION fn::hvac_width_three($pe: record){
    let $is_circle = fn::hvac_shape($pe) == 'CIRC';
    return if $is_circle {
            return [string::concat('φ', fn::hvac_width($pe)), string::concat( 'φ', fn::hvac_width2($pe) ),
                string::concat( 'φ', fn::hvac_width3($pe)) ];
        } else {
            return [fn::hvac_width($pe), fn::hvac_width2($pe), fn::hvac_width3($pe)];
        };
};

DEFINE FUNCTION fn::hvac_height_three($pe: record){
    let $is_circle = fn::hvac_shape($pe) == 'CIRC';
    return if $is_circle {
            return ['-', '-', '-'];
        } else {
            return  [fn::hvac_height($pe), fn::hvac_height2($pe), fn::hvac_height3($pe)];
        };
};

DEFINE FUNCTION fn::hvac_width($pe: record, $n: option<int>){
    return if $n == none || $n == 1 {
        return $pe.refno.DESP[1];
    } else if $n == 2 {
        return fn::hvac_width2($pe);
    } else if $n == 3 {
        return fn::hvac_width3($pe);
    };
};


DEFINE FUNCTION fn::hvac_width2($pe: record){
    return $pe.refno.DESP[3];
};

DEFINE FUNCTION fn::hvac_width3($pe: record){
    return $pe.refno.DESP[40];
};


DEFINE FUNCTION fn::hvac_height($pe: record, $n: option<int>){
    return if $n == none || $n == 1 {
        return $pe.refno.DESP[2];
    } else if $n == 2 {
        return fn::hvac_height2($pe);
    } else if $n == 3 {
        return fn::hvac_height3($pe);
    };
};

DEFINE FUNCTION fn::hvac_height_is_DIAM($pe: record){
    return fn::hvac_height($pe) == 788296;
};

DEFINE FUNCTION fn::hvac_height2($pe: record){
    return $pe.refno.DESP[4];
};

DEFINE FUNCTION fn::hvac_height2_is_DIAM($pe: record){
    return fn::hvac_height2($pe) == 788296;
};

DEFINE FUNCTION fn::hvac_height3($pe: record){
    return $pe.refno.DESP[41];
};

DEFINE FUNCTION fn::hvac_len($pe: record){
    return $pe.refno.DESP[5];
};

DEFINE FUNCTION fn::hvac_len_format($pe: record){
    return math::fixed(fn::hvac_len($pe), 2);
};

DEFINE FUNCTION fn::hvac_a_len($pe: record){
    return $pe.refno.DESP[14];
};

DEFINE FUNCTION fn::hvac_len_A($pe: record){
    let $type = fn::type($pe);
    return if $type == 'BRCO' {
        return $pe.refno.DESP[45];
    } else {
        return $pe.refno.DESP[14];
    };
};

DEFINE FUNCTION fn::hvac_len_L($pe: record){
    let $type = fn::type($pe);
    return if $type == 'BRCO' {
        return $pe.refno.DESP[18];
    } else {
        return $pe.refno.DESP[16];
    };
};


DEFINE FUNCTION fn::hvac_fl_width($pe: record, $n: option<int>){
    return if $n == none || $n == 1 {
        return $pe.refno.DESP[20];
    } else if $n == 2 {
        return fn::hvac_fl_width2($pe);
    } else if $n == 3 {
        return fn::hvac_fl_width3($pe);
    };
};


DEFINE FUNCTION fn::hvac_fl_width2($pe: record){
    return $pe.refno.DESP[23];
};

DEFINE FUNCTION fn::hvac_fl_width3($pe: record){
    return $pe.refno.DESP[26];
};



DEFINE FUNCTION fn::hvac_thk($pe: record, $chk_type: option<string>){
    return if $chk_type == "DAMP" {
          let $mat = fn::hvac_mat_number($pe);
          let $p_type = fn::prev_type($pe);
          let $n_type = fn::next_type($pe);
          return if $mat == 6 && ( $p_type == "DAMP" || $n_type == "DAMP" ) {
                  return [$pe.refno.DESP[4], '', 3];
          } else {
              return $pe.refno.DESP[4];
          };
    } else {
        return $pe.refno.DESP[4];
    };
};

DEFINE FUNCTION fn::duct_area($pe: record){
    return $pe.refno.DESP[80];
};


DEFINE FUNCTION fn::hvac_duct_areas($pe: record, $chk_type: option<string>){
    let $ispec = fn::get_ipara($pe)[0];
    let $area = fn::hvac_duct_area_format($pe, $chk_type);
    return if $ispec > 0 {
       return [ $area, $area ];
    } else {
       return [ $area ];
    };
};



DEFINE FUNCTION fn::duct_weight($pe: record){
    return $pe.refno.DESP[84];
};

DEFINE FUNCTION fn::hvac_duct_weight_format($pe: record, $chk_type: option<string>){
    let $weight = math::fixed(fn::duct_weight($pe), 2);
    return if $chk_type == "DAMP" {
          let $mat = fn::hvac_mat_number($pe);
          let $pd = fn::pre_is_type($pe, 'DAMP');
           let $nd = fn::next_is_type($pe, 'DAMP');
           return if $mat == 6 && ( $pd || $nd ) {
                let $new_weight = (fn::height($pe) * fn::width($pe) * fn::hvac_thk($pe) * 150 * 7.85 ) / 1000000;
                return [$weight, '', math::fixed($new_weight, 2)];
          } else {
              return $weight;
          };
    } else {
        return $weight;
    };
};


DEFINE FUNCTION fn::hvac_max_size($pe: record){
    return math::max([$pe.refno.DESP[1], $pe.refno.DESP[2]]);
};

DEFINE FUNCTION fn::hvac_pressure($pe: record){
    return array::at(string::split(($pe->pe_owner->pe.name)[0], '-'), 4);
};

DEFINE FUNCTION fn::hvac_shape_with_pressure($pe: record){
    return string::join('/', fn::hvac_shape($pe), fn::hvac_pressure($pe));
};



insert into hvac_mat [
    {
        "id": 1,
        "code": "GS",
        "desc": "镀锌风管",
    },{
        "id": 2,
        "code": "CS",
        "desc": "碳钢风管",
    },{
        "id": 3,
        "code": "ACS",
        "desc": "气密性碳钢风管",
    },{
        "id": 4,
        "code": "SS",
        "desc": "不锈钢风管",
    },{
        "id": 5,
        "code": "GS",
        "desc": "气密性不锈钢风管",
    },{
        "id": 6,
        "code": "FP",
        "desc": "防火风管",
    }
];

DEFINE FUNCTION fn::hvac_mat($pe: record){
    let $m = (select value code from fn::hvac_mat_id($pe))[0];
    return if $m == none {
        return "unset";
    } else {
        return $m;
    };
};

//暂时返回数字
DEFINE FUNCTION fn::stype($pe:record) {
    return $pe.refno.DESP[0];
};

DEFINE FUNCTION fn::prev_stype($pe:record) {
    let $p = fn::prev_pe($pe);
    return if $p == none {
        return 'unset';
    } else {
         return fn::stype($p);
    };
};

DEFINE FUNCTION fn::next_stype($pe:record) {
    let $n = fn::next_pe($pe);
    return if $n == none {
        return 'unset';
    } else {
       return fn::stype($n);
    };
};

//C_Stype="DESP[1] OF CREF"
DEFINE FUNCTION fn::c_stype($pe:record) {
    return $pe.refno.CREF.refno.DESP[0];
};

//cur
DEFINE FUNCTION fn::stype_is_fjfd($pe:record) {
    return fn::stype($pe) == 614823;
};

DEFINE FUNCTION fn::stype_is_damp($pe:record) {
    return fn::stype($pe) == 855877;
};

DEFINE FUNCTION fn::stype_is_mesh($pe:record) {
    return fn::stype($pe) == 702904;
};

//prev
DEFINE FUNCTION fn::prev_stype_is_fjfd($pe:record) {
    return fn::prev_stype($pe) == 614823;
};

DEFINE FUNCTION fn::prev_stype_is_damp($pe:record) {
    return fn::prev_stype($pe) == 855877;
};

DEFINE FUNCTION fn::prev_stype_is_mesh($pe:record) {
    return fn::prev_stype($pe) == 702904;
};

//next
DEFINE FUNCTION fn::next_stype_is_fjfd($pe:record) {
    return fn::next_stype($pe) == 614823;
};


DEFINE FUNCTION fn::c_stype_is_fjfd($pe:record) {
    return fn::c_stype($pe) == 614823;
};

DEFINE FUNCTION fn::next_stype_is_damp($pe:record) {
    return fn::next_stype($pe) == 855877;
};

DEFINE FUNCTION fn::next_stype_is_mesh($pe:record) {
    return fn::next_stype($pe) == 702904;
};


DEFINE FUNCTION fn::hvac_duct_area_format($pe: record, $chk_type: option<string>){
    // 1000000
    let $area = math::fixed(fn::duct_area($pe) , 2);
    return if $chk_type == 'DAMP' {
          let $mat = fn::hvac_mat_number($pe);
          let $pd = fn::pre_is_type($pe, 'DAMP');
          let $nd = fn::next_is_type($pe, 'DAMP');
          return if $mat == 6 && ( $pd || $nd ) {
                let $new_area = fn::height($pe) * fn::width($pe) * 150 / 1000000;
                return [$area, '', math::fixed($new_area, 2)];
          } else {
              return $area;
          };
    } else {
        return $area;
    };
};
