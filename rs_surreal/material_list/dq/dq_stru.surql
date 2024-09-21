-- 电气专业的支吊架
remove function fn::dq_stru;
define function fn::dq_stru($pes:array<record>){  
    return select
        fn::pe_to_refno(id) as id, // 参考号
        string::slice(fn::find_ancestor_type(id,'SITE')[0].name?:'',1,1) as num, // 机组号
        string::slice((string::split(fn::find_ancestor_type(id,'SITE')[0].name?:'', '-'))[0]?:'',2) as project_num, //子项号
        fn::find_ancestor_type(id,'SITE')[0].refno.DESC?:'' as project_name, //子项名称
        '支吊架' as major,//专业
        fn::room_code(id)[0]?: '' as room_code, // 房间号
        string::slice(refno.NAME?:'',1) as supp_name, // 托盘支吊架名称
        '碳钢Q355' as material,  //材质
        array::last(string::split((<-pe_owner.in<-pe_owner[where in.noun='SCTN'|| in.noun ='GENSEC'].in.refno.SPRE.name)[0]?:'-','-')) as size_num, // 规格型号
        (<-pe_owner.in<-pe_owner[where in.noun='SCTN'|| in.noun ='GENSEC'].in.refno.SPRE.name)[0]?:'' as spre,
        (<-pe_owner.in<-pe_owner[where in.noun='SCTN'|| in.noun ='GENSEC'].in.refno.SPRE.refno.CATR.refno.NAME)[0]?:'' as catr,
        '2' as count // 数量
    from $pes where !string::contains(fn::find_ancestor_type(id,'ZONE')[0].name?:'','MTGD');
};


-- let $pe = [pe:25688_76282];
-- return fn::dq_stru($pe);