remove function fn::fggd_bran_code;
define function fn::fggd_bran_code($pe:record) {
    return select value string::concat(string::split(owner.name?:'','-')[2]?:'',
    string::split(string::split(owner.name?:'','-')[5]?:'',' ')[0],
    '/',
    string::slice(fn::get_uda_value(id, '/H-SECTCODE')?:'',-5)) as bran_seg_code_split from $pe
};

// return fn::fggd_bran_code(pe:24384_24936);