//加强筋型材 和 长度
//exp0: 0
//exp1: ([Width] + [Height] + 83) * 2 / 1000
//exp2: ([Width] + [Height] + 103) * int([Length] / 800 - 0.5) * 2 / 1000
insert into stif_table  [

    //RECT/L
     {
        "id": "0",
        "type": "STRT",
        "pattern": "RECT/L",
        "max_size_range": [0, 800],
        "stif_sctn": "L40x4",
        "stiff_len": "exp0"
    },
    {
        "id": "1",
        "type": "STRT",
        "pattern": "RECT/L",
        "max_size_range": [800, 2000],
        "exclude_mat": hvac_mat:6,
        "stif_sctn": "L40x4",
        "stiff_len": "exp1"
    },
    {
        "id": "2",
        "type": "STRT",
        "pattern": "RECT/L",
        "max_size_range": [2000, 100000],
        "stif_sctn": "L50x5",
        "stiff_len": "exp2"
    },

    //RECT/M
     {
        "id": "3",
        "type": "STRT",
        "pattern": "RECT/M",
        "max_size_range": [0, 800],
        "exclude_mat": hvac_mat:6,
        "stif_sctn": "-",
        "stiff_len": "exp0"
    },
     {
        "id": "4",
        "type": "STRT",
        "pattern": "RECT/M",
        "max_size_range": [800, 2000],
        "exclude_mat": hvac_mat:6,
        "stif_sctn": "L40x4",
        "stiff_len": "exp1"
    },
     {
        "id": "5",
        "type": "STRT",
        "pattern": "RECT/M",
        "max_size_range": [2000, 100000],
        "stif_sctn": "L50x5",
        "stiff_len": "exp2"
    },

    //RECT/H
      {
        "id": "6",
        "type": "STRT",
        "pattern": "RECT/H",
        "max_size_range": [0, 600],
        "stif_sctn": "-",
        "stiff_len": "exp0"
    },
     {
        "id": "7",
        "type": "STRT",
        "pattern": "RECT/H",
        "max_size_range": [600, 1800],
        "exclude_mat": hvac_mat:6,
        "stif_sctn": "L40x4",
        "stiff_len":  "exp1"
    },
    {
        "id": "8",
        "type": "STRT",
        "pattern": "RECT/H",
        "max_size_range": [1800, 100000],
        "stif_sctn": "L50x5",
        "stiff_len": "exp2"
    }
];

insert into stif_wei  [
     {
        "id": "0",
        "type": "STRT",
        "shapes": ["RECT/L", "RECT/M"],
        "max_size_range": [0, 2000],
        "scale": 2.422
    },{
        "id": "1",
        "type": "STRT",
        "shapes": ["RECT/L", "RECT/M"],
        "max_size_range": [2000, 100000],
        "scale": 3.77
    },
   {
        "id": "2",
        "type": "STRT",
        "shapes": ["RECT/H"],
        "max_size_range": [0, 1800],
        "scale": 2.422
    },{
        "id": "3",
        "type": "STRT",
        "shapes": ["RECT/H"],
        "max_size_range": [1800, 100000],
        "scale": 3.77
    },
];

//计算加强筋型材
DEFINE FUNCTION fn::cal_stif_sctn($pe: record) {
    let $mat_id = fn::hvac_mat_id($pe);
    let $max_size = fn::hvac_max_size($pe);
    let $s = fn::hvac_shape_with_pressure($pe);
	RETURN (select value stif_sctn from stif_table where $max_size >= max_size_range[0] and $max_size < max_size_range[1] and $s=pattern and ($mat_id not in exclude_mat))[0];
};

DEFINE FUNCTION fn::cal_stif_sctn_str($pe: record) {
    let $s = fn::cal_stif_sctn($pe);
    return if $s == none {
               '-'
           } else {
               $s
    };
};


//计算加强筋长度
DEFINE FUNCTION fn::cal_stif_len($pe: record) {
    let $mat_id = fn::hvac_mat_id($pe);
    let $w = fn::hvac_width($pe);
    let $h = fn::hvac_height($pe);
    let $l = fn::hvac_len($pe);
    let $max_size = math::max([$w, $h]);
    let $s = fn::hvac_shape_with_pressure($pe);
    let $expr = (select value stiff_len from stif_table where $max_size >= max_size_range[0] and $max_size < max_size_range[1] and $s=pattern and ($mat_id not in exclude_mat))[0];

    return  if $expr=='exp0' {
               0
           } else if $expr=='exp1' {
               (($w + $h + 83) * 2 )/ 1000
           } else if $expr=='exp2' {
               (($w + $h + 103) * math::round($l / 800 - 0.5) * 2) / 1000
           };
};


DEFINE FUNCTION fn::cal_stif_len_str($pe: record) {
    let $s = fn::cal_stif_len($pe);
    return if $s == none {
               0
           } else {
               $s
    };
};


DEFINE FUNCTION fn::cal_stif_wei($pe: record) {
    let $max_size = fn::hvac_max_size($pe);
    let $shape = fn::hvac_shape_with_pressure($pe);
    let $scale = (select value scale from stif_wei where $shape in shapes and  $max_size >= max_size_range[0] and $max_size < max_size_range[1])[0];
    return $scale * fn::cal_stif_len($pe);
};

DEFINE FUNCTION fn::cal_stif_wei_str($pe: record) {
    let $s = fn::cal_stif_len($pe);
    return if $s == none {
               0
           } else {
               $s
    };
};



DEFINE FUNCTION fn::cal_stif_sctn_STIF($pe: record) {
    return string::concat('L', fn::hvac_fl_width($pe), 'X', fn::hvac_fl_width2($pe), 'X', fn::hvac_fl_width3($pe) );
};

DEFINE FUNCTION fn::cal_stif_len_STIF($pe: record) {
    let $is_circle = fn::hvac_shape($pe) == 'CIRC';
    return if $is_circle {
        return ( fn::hvac_width($pe)  + fn::hvac_fl_width($pe) * 2.0 ) * 3.14 / 1000.0;
    } else {
        return ( fn::hvac_width($pe) + fn::hvac_height($pe) + fn::hvac_fl_width($pe) * 2.0 ) * 2.0 / 1000.0;
    };
};

//加强筋型材
DEFINE FUNCTION fn::cal_stif_wei_STIF($pe: record) {
    return math::fixed( fn::cal_stif_len_STIF($pe) *
        ( fn::hvac_fl_width($pe) + fn::hvac_fl_width2($pe) - fn::hvac_fl_width3($pe) ) *
        fn::hvac_fl_width3($pe) * 0.00785, 2);
};



