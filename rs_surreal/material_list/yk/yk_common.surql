// 对应根阀编号
DEFINE FUNCTION fn::yk_pipe_num($pe:record) {
        let $zone_name = ($pe->pe_owner.out->pe_owner.out.name)[0];
        let $names = string::split($zone_name,'-');
        let $pipe_name = if array::len(string::split($zone_name,'-')) < 3 { null } else { string::concat($names[0],'-',$names[2],'-','PIPE')};
        return
        if string::contains($zone_name,'INST') && $pipe_name != NONE {
            (select <-pe_owner.in<-pe_owner.in<-pe_owner[where in.noun='INST'].out.refno.HREF.refno.NAME as href from pe where name = $pipe_name)
        } else {
            ''
        }
};


// 找到根阀, 通过valv或者inst所在的bran，一直查找href，
// 一般最后仪控的管道连接的是工艺的valv，判断最后连接的valv是否是工艺专业，若是则返回这个工艺valv所在的bran的name

//todo 重构这个方法
DEFINE FUNCTION fn::find_gy_bran($pe:record) {
    let $first_level = $pe->pe_owner.out.refno.HREF->pe_owner.out;
    let $second_level = $pe->pe_owner.out.refno.HREF.refno.HREF->pe_owner.out;
    let $third_level = $pe->pe_owner.out.refno.HREF.refno.HREF.refno.HREF->pe_owner.out;
    let $forth_level = $pe->pe_owner.out.refno.HREF.refno.HREF.refno.HREF.refno.HREF->pe_owner.out;
    let $fivth_level = $pe->pe_owner.out.refno.HREF.refno.HREF.refno.HREF.refno.HREF.refno.HREF->pe_owner.out;

    let $first_site = $first_level->pe_owner.out->pe_owner.out->pe_owner.out.name;
    let $second_site = $second_level->pe_owner.out->pe_owner.out->pe_owner.out.name;
    let $third_site = $third_level->pe_owner.out->pe_owner.out->pe_owner.out.name;
    let $forth_site = $forth_level->pe_owner.out->pe_owner.out->pe_owner.out.name;
    let $fivth_site = $fivth_level->pe_owner.out->pe_owner.out->pe_owner.out.name;

    return
    if $first_site[0][0] != NONE && string::contains($first_site[0][0],'PIPE') {
        return select value $first_level.refno.NAME as r from $pe
    } else if $second_site[0][0] != NONE && string::contains($second_site[0][0],'PIPE') {
        return select value $second_level.refno.NAME as r from $pe
    } else if $third_site[0][0] != NONE && string::contains($third_site[0][0],'PIPE') {
        return select value $third_level.refno.NAME as r from $pe
    } else if $forth_site[0][0] != NONE && string::contains($forth_site[0][0],'PIPE') {
        return select value $forth_level.refno.NAME as r from $pe
    } else if $fivth_site[0][0] != NONE && string::contains($fivth_site[0][0],'PIPE') {
        return select value $fivth_level.refno.NAME as r from $pe
    }
};