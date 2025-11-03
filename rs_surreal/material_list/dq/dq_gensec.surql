// 电气 托盘及接地
// gensec

remove function fn::dq_gensec;
define function fn::dq_gensec($pes:array<record>){    
    return select 
        fn::pe_to_refno(id) as id, // 参考号
        string::slice(fn::find_ancestor_type(id,'SITE')[0].name?:'',1,1) as num, // 机组号
        string::slice((string::split(fn::find_ancestor_type(id,'SITE')[0].name?:'', '-'))[0]?:'',2) as project_num, //子项号
        fn::find_ancestor_type(id,'SITE')[0].refno.DESC?:'' as project_name, //子项名称
        '主托盘接地' as major,//专业
        fn::room_code(id)[0]?:'' as room_code, // 房间号
        string::slice(refno.NAME?:'',1) as name, // 托盘段号
        math::fixed(refno.POS[2]?:0,3) as pos,
        '裸铜缆' as material,  //材质
        refno.SPRE.refno.NAME?:'' as spre,  
        refno.SPRE.refno.CATR.refno.NAME?:'' as catr,
        if array::len(array::clump(<-pe_owner.in<-pe_owner[where in.noun='POINSP'].in.refno.POS,3)) == 2 {
            math::fixed(fn::array_distance(array::clump(<-pe_owner.in<-pe_owner[where in.noun='POINSP'].in.refno.POS,3)), 3)
        } else {
            0.0
        } as length
    from $pes where string::contains(fn::find_ancestor_type(id,'ZONE')[0].name?:'','MTGD');
};

-- let $pe = [pe:25688_76336];
-- return fn::dq_gensec($pe);
