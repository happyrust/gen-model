-- 查找子集
remove function fn::find_group_sube_children;
DEFINE FUNCTION fn::find_group_sube_children($pe:record) {
    let $children = $pe<-pe_owner[where in.noun = 'SUBE'].in;
    return $children<-pe_owner.in.refno;
};

remove function fn::find_group_box_size_and_high_z;
DEFINE FUNCTION fn::find_group_box_size_and_high_z($pe:record) {
    let $r = select value in from $pe<-pe_owner;
    let $b = (select value in from $r<-pe_owner where array::at(array::distinct(in.noun), 0)='BOX'
            and array::len(array::distinct(in.noun))=1 and array::len(in) in [3, 4])[0]?:[];

    let $max_size = array::max(array::flatten(select value trans.d.scale
                from array::flatten(array::flatten(($b->inst_relate.out->geo_relate)))));
    let $high_z = array::max(array::flatten(select value world_trans.d.translation[2]
                from array::flatten($b->inst_relate)));
    return [$max_size, $high_z];
};


-- let $pe = [pe:24383_83477];
-- return fn::find_group_sube_children($pe);
