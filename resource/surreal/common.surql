
-- DEFINE FIELD refnos ON TABLE his_pe TYPE set<record<pe>>;

REMOVE FUNCTION fn::ancestor;
-- 考虑到版本的原因，需要使用到历史祖先的记录，所以需要使用到这个函数
DEFINE FUNCTION fn::ancestor($r: record) {
    return if $r.id == none {
        return [];
    } else {
        let $a = SELECT value [id,
            owner ,
            owner.owner,
            owner.owner.owner,
            owner.owner.owner.owner,
            owner.owner.owner.owner.owner ,
            owner.owner.owner.owner.owner.owner,
            owner.owner.owner.owner.owner.owner.owner,
            owner.owner.owner.owner.owner.owner.owner.owner,
            owner.owner.owner.owner.owner.owner.owner.owner.owner]
            from only $r limit 1;
        return array::complement($a,
            [NONE, pe:0_0]
        );
    };
};

REMOVE FUNCTION fn::newest_pe;
DEFINE FUNCTION fn::newest_pe(
    $pe: record,
) {
    if $pe == none {
        return none;
    };
    let $id = record::id($pe);
    if !type::is::array($id) {
        return $pe;
    };
    let $refno = $id[0];
    return type::thing('pe', $refno);
};

REMOVE FUNCTION fn::newest_pe_id;
DEFINE FUNCTION fn::newest_pe_id(
    $pe: record,
) {
    if !type::is::array($pe) {
        return record::id($pe);
    };
    return record::id($pe)[0];
};


REMOVE FUNCTION fn::order;
DEFINE FUNCTION fn::order(
    $pe: record,
) {
    if $pe.owner == none {
        return none;
    };
    let $a = ($pe.owner<-pe_owner.in)[where noun=$pe.noun];
    if $a == none {
        let $b = (fn::newest_pe($pe).owner<-pe_owner.in)[where noun=$pe.noun];
        return array::find_index($b, $pe);
    } else {
        return array::find_index($a, $pe);
    };
};

REMOVE FUNCTION fn::default_name;
DEFINE FUNCTION fn::default_name(
    $pe: record,
) {
    return (select value (if name=='' {string::join(' ', noun, <string> (fn::order($pe)?:0 + 1) ) } else { name } ) from $pe)[0];
};

remove function fn::default_names;
DEFINE FUNCTION fn::default_names(
    $pes: array<record>,
) {
    return select value (if name=='' {string::join(' ', noun,<string> (
       fn::order(id)?:0 + 1) ) } else { name } ) from $pes;
};


REMOVE FUNCTION fn::default_full_name;
DEFINE FUNCTION fn::default_full_name($r: record) {
    return if !record::exists($r) {
        return '';
    }else if $r.name != '' && $r.name != none {
        return $r.name;
    } else if $r.owner.name != '' && $r.owner.name != none {
        return string::concat(fn::default_name($r), " OF ", $r.owner.noun, ' ', $r.owner.name);
    } else {
        let $a = fn::ancestor($r);
        let $b = array::len($a) - array::find_index(array::reverse($a).name, '');
        let $i = array::slice($a, 0, $b);
        let $c = $b + 1;
        return string::concat(array::join(fn::default_names($i), " OF "), " OF ", $a[$c].noun, ' ', $a[$c].name);
    };
};


REMOVE FUNCTION fn::backup_data;
//备份修改的数据，只针对修改的处理
//如果是新增，应该是对pe_owner的备份
DEFINE FUNCTION fn::backup_data(
    $pes: array<record>,
    $is_del: bool,
    $sesno: number,
) {
    for $pe in $pes {
        //只操作最新的 refno
        if type::is::array(record::id($pe)) || !record::exists($pe) {
            continue;
        };
        if $pe.sesno > $sesno {
            continue;
        };
        let $old_id = [<string> record::id($pe.id), $pe.sesno];
        let $old_pe_id = type::thing('pe', $old_id);

        let $a = (select * omit id from $pe)[0];
        let $b = (select * omit id from $pe.refno)[0];

        if $a != none && $b != none {
            let $old_inst_relate_id = type::thing('inst_relate', $old_id);
            let $old_att_id = type::thing(string::concat($a.noun, '_H'), $old_id);
            let $old_uda_att_id = type::thing('ATT_UDA_H', $old_id);
            if $is_del{
                //软删除
                update $pe.id set op=2, old_pe=$old_pe_id, sesno=$sesno, deleted=true;
            } else {
                update $pe.id set op=1, old_pe=$old_pe_id, sesno = $sesno;
            };
            upsert type::thing('his_pe', record::id($pe.id)) set refnos+=$old_pe_id;
            upsert $old_pe_id content $a.patch([{
                                		'op': 'replace',
                                		'path': 'refno',
                                		'value': $old_att_id
                                }]);
            //属性数据需要单独的备份方法，因为有很多引用
            upsert $old_att_id content $b.patch([{
                                		'op': 'replace',
                                		'path': 'REFNO',
                                		'value': $old_pe_id
                                }]);
            //uda 属性需要单独的备份方法，因为有很多引用
            let $uda_data = select * from only type::thing('ATT_UDA', record::id($pe.id)) limit 1;
            if $uda_data != none {  
                upsert $old_uda_att_id content $uda_data.patch([{
                                		'op': 'replace',
                                		'path': 'refno',
                                		'value': $old_pe_id
                                }]);
            };
            //删除属性
            if $is_del{
               delete $pe.refno;
            };

            //备份关联
            let $c = (select *, $old_inst_relate_id as id, $old_pe_id as in from $pe->inst_relate);
            if !array::is_empty($c) {
                delete select id from $pe->inst_relate;
                insert relation into inst_relate $c;
            };

            if $a.noun == "BRAN" || $a.noun == "HANG" {

                let $tubis = (select *, type::thing('tubi_relate', [ $old_pe_id, record::id(id)[1] ]) as id,
                                 $old_pe_id as in from $pe->tubi_relate);
                //将 tubi relate 整体备份
                if !array::is_empty($tubis) {
                    //删除之前的，变成历史的
                    delete select id from $pe->tubi_relate;
                    insert relation into tubi_relate $tubis;
                };
            };
        }
    }
};


REMOVE FUNCTION fn::backup_owner_relate;
//备份修改的数据，只针对修改的处理
//如果是新增，应该是对pe_owner的备份
DEFINE FUNCTION fn::backup_owner_relate(
    $pes: array<record>,
    $delete_old: bool,
) {
    for $pe in $pes {
        //只操作最新的 refno
        // if type::is::array(record::id($pe)) || !record::exists($pe) {
        //      continue;
        // };
        let $old_id = [<string> record::id($pe.id), $pe.sesno];
        let $old_pe_id = type::thing('pe', $old_id);
        //fix 层级不连续的问题，需要更新到之前的 in->owner 关系
        //let $old_owners_id = select value od from $pe->pe_owner where type::is::array(record::id(out));
        -- return $old_owners_id;
        //如果是旧节点，需要更新到旧的引用节点
        //for $o in $old_owners_id {
            //需要删除了，重新创建这个单独的id
            //update $o set in=$old_pe_id;
        //};
        //如果子节点的sesno >= 父节点的sesno 了，则需要使用 old_pe
        //反之则不需要动
        let $c = select *, type::thing('pe_owner', [ $old_pe_id, record::id(id)[1] ]) as id,
                    if $pe.sesno>in.sesno { in } else { in.old_pe?:in } as in, $old_pe_id as out
                    from $pe<-pe_owner;
        -- return $c;
        if !array::is_empty($c) {
            if $delete_old{
                delete select id from $pe<-pe_owner;
            };
            insert relation into pe_owner $c;
        }
    }
};

// let $pe = pe:17496_171606;
// fn::backup_owner_relate([$pe], false);

remove function fn::latest_pe_sesno;
define function fn::latest_pe_sesno($pe: record, $s: option<number>, $db: option<number>){
    let $sesno = $pe.sesno?:0;
    let $r = return if $s != none and $db != none {
        #找到对应的时间
        let $t = ses:[$db, $s].date;
        return select value sesno from only pe_ses_h:[record::id($pe), 0]..[record::id($pe)] where ses.date<=$t ORDER BY sesno DESC limit 1;
    } else if $s != none {
        return if $s >= $sesno {
            return $sesno;
        } else{
            return select value sesno from only pe_ses_h:[record::id($pe), 0]..[record::id($pe)] where sesno<=$s ORDER BY sesno DESC limit 1;
        };
    } else {
        return $sesno
    };

    return [$r?:0, $sesno];
};

remove function fn::latest_pe;
define function fn::latest_pe($pe: record, $s: option<number>, $db: option<number>){
    let $s = fn::latest_pe_sesno($pe, $s, $db);
    return if $s[1]  {
        return $pe;
    } else{
        let $id = record::id($pe);
    return type::thing('pe', [$id, $s[0]]);
    }
};


remove function fn::find_pe_by_datetime;
define function fn::find_pe_by_datetime($pe: record, $dt: datetime){
    let $his_pe = type::thing('his_pe', fn::newest_pe_id($pe));
    if !record::exists($his_pe) {
        return $pe;
    };
    let $a = select value id from only array::reverse(array::flatten([$his_pe.refnos?:[], $pe])) where <datetime>fn::ses_date(id) <= $dt limit 1;
    return $a?:$pe;
};


-- fn::latest_pe_sesno(pe:17496_171715, 880, 1112);
-- fn::latest_pe_sesno(pe:17496_171715, 880);
-- fn::latest_pe_sesno(pe:17496_171715);

remove function fn::ses_data;
define function fn::ses_data($pe: record){
    let $id = type::thing('ses', [$pe.dbnum, $pe.sesno]);
    return select * from only $id limit 1;
};

remove function fn::ses_date;
define function fn::ses_date($pe: record){
    return if record::exists($pe) {
        let $id = type::thing('ses', [$pe.dbnum, $pe.sesno]);
        return select value date from only $id limit 1;
    } else {
        return none;
    };
};

remove function fn::next_is_type;
DEFINE FUNCTION fn::next_is_type($pe:record,$type:string) {
    return fn::next_type($pe)==$type;
};

remove function fn::first_child;
DEFINE FUNCTION fn::first_child($pe:record) {
    return select value (<-pe_owner[0].in) from only $pe limit 1;
};

remove function fn::first_child_exclude_type;
DEFINE FUNCTION fn::first_child_exclude_type($pe:record, $exclude_type: string) {
    return select value in from only $pe<-pe_owner where in.noun != $exclude_type limit 1;
};

remove function fn::last_child;
DEFINE FUNCTION fn::last_child($pe:record) {
    return array::last((select value (<-pe_owner.in) from only $pe limit 1)?:[]);
};

remove function fn::last_child_exclude_type;
DEFINE FUNCTION fn::last_child_exclude_type($pe:record, $exclude_type: string) {
    return (select * from only $pe<-pe_owner where in.noun != $exclude_type order by id desc limit 1).in;
};

remove function fn::prev_pe;
DEFINE FUNCTION fn::prev_pe($pe:record) {
    if $pe == none || !record::exists($pe) {
                return none;
        };
        let $a = (record::id($pe->pe_owner.id[0])[1]);
        return (select value in from ($pe->pe_owner->pe<-pe_owner)[..$a]).last();
};

remove function fn::prev_pe_exclude_type;
DEFINE FUNCTION fn::prev_pe_exclude_type($pe:record, $exclude_type: string) {
    if $pe == none || !record::exists($pe) {
            return none;
    };
    let $a = (record::id($pe->pe_owner.id[0])[1]);
    return (select value in from ($pe->pe_owner->pe<-pe_owner)[..$a] where in.noun != $exclude_type).last();
};

remove function fn::last_child_exclude_type;
DEFINE FUNCTION fn::last_child_exclude_type($pe:record, $exclude_type: string) {
    return (select * from only $pe<-pe_owner where in.noun != $exclude_type order by id desc limit 1).in;
};

remove function fn::next_pe;
DEFINE FUNCTION fn::next_pe($pe:record) {
     if $pe == none || !record::exists($pe) {
                return none;
            };
        let $a = (record::id($pe->pe_owner.id[0])[1]) + 1;
        return select value in from only ($pe->pe_owner->pe<-pe_owner)[$a..] limit 1;
};

remove function fn::next_pe_exclude_type;
DEFINE FUNCTION fn::next_pe_exclude_type($pe:record, $exclude_type: string) {
    if $pe == none || !record::exists($pe) {
            return none;
        };
    let $a = (record::id($pe->pe_owner.id[0])[1]) + 1;
    return select value in from only ($pe->pe_owner->pe<-pe_owner)[$a..] where in.noun != $exclude_type  limit 1;
};

remove function fn::prev_connect_pe;
DEFINE FUNCTION fn::prev_connect_pe($pe:record) {
    if $pe == none {
        return none;
    };
    let $p = fn::prev_pe_exclude_type($pe, 'ATTA');
    return if $p != none {
        return $p;
    } else {
        let $connect_bran = $pe.owner.refno.HREF;
        return if $connect_bran != none {
            return fn::last_child($connect_bran)?:$connect_bran;
        } else {
            return none;
        };
    };
};

remove function fn::prev_connect_pe_data;
DEFINE FUNCTION fn::prev_connect_pe_data($pe:record) {
    let $p = fn::prev_connect_pe($pe);
    //如果有到达这个元件的直段，则返回这个直段的leave
    let $tmp_tubi_refno = fn::query_tubi_from($pe).leave;
    let $tubi_refno = if $tmp_tubi_refno != none {
        $tmp_tubi_refno
    } else if fn::query_tubi_to($pe.owner) != none {   //判断bran的最尾部是不是TUBI
        // 返回是否有bran开启的直段
        fn::query_tubi_to($pe.owner)
    } else {
       none
    };
    let $tubi_name = return if $tubi_refno == none {
        none
    } else { string::concat('ileave tube of ',  fn::default_full_name($tubi_refno)) };
    return if $p != none {   //如果存在上一个节点
        //如果tubi_refno是branch的最后一个节点，则需要判断关联的branch有tubi
        let $tubi_spref = return if $tubi_refno == none {
            none
        } else if $tubi_refno.noun == 'BRAN'{
            $tubi_refno.refno.HSTU.name
        } else {
            $tubi_refno.refno.LSTU.name
        };
        return {
            id: $pe,
            prev_full_name: fn::default_full_name($p),
            prev: $p,
            tubi_name: $tubi_name,
            tubi_refno: $tubi_refno,
            tubi_spref: $tubi_spref
        };
    } else {  //如果不存在上一个节点
        return if $tubi_refno != none {
            return {
                id: $pe,
                prev_full_name: fn::default_full_name($pe.owner),
                prev: $pe.owner,
                tubi_name: $tubi_name,
                tubi_refno: $pe.owner,
                tubi_spref: $pe.owner.refno.HSTU.name
            };
        } else {
            none
        };
    };
};

remove function fn::next_connect_pe;
DEFINE FUNCTION fn::next_connect_pe($pe:record) {
    if $pe == none {
        return none;
    };
    let $p = fn::next_pe_exclude_type($pe, 'ATTA');
    return if $p != none {
        return $p;
    } else {
        let $connect_bran = $pe.owner.refno.TREF;
        return if $connect_bran != none {
            return fn::first_child($connect_bran)?:$connect_bran;
        } else {
            return none;
        };  
    };
};

remove function fn::next_connect_pe_data;
DEFINE FUNCTION fn::next_connect_pe_data($pe:record) {
    let $p = fn::next_connect_pe($pe);
    let $tubi_refno = if fn::query_tubi_to($pe) != none {
        $pe
    } else if fn::query_tubi_to($pe.owner.refno.TREF) != none {
        // 返回是否有bran开启的直段
      $pe.owner.refno.TREF
    } else {
       none
    };
    let $tubi_name = return if $tubi_refno == none {
       none
   } else { string::concat('ileave tube of ',  fn::default_full_name($pe)) };
    return if $p != none {
        let $tubi_spref = return if $tubi_refno == none {
            none
        } else if $tubi_refno.noun == 'BRAN'{
            $tubi_refno.refno.HSTU.name
        } else {
            $tubi_refno.refno.LSTU.name
        };
        return {
            id: $pe,
            next_full_name: fn::default_full_name($p),
            next: $p,
            tubi_refno: $tubi_refno,
            tubi_name: $tubi_name,
            tubi_spref: $tubi_spref
        };
    } else {
        return if $tubi_refno != none {
            return {
                id: $pe,
                next_full_name: fn::default_full_name($pe),
                next: $pe,
                tubi_refno: tubi_refno,
                tubi_name: $tubi_name,
                tubi_spref: $pe.refno.LSTU.name           
            };
        } else {
            none
        };
    };
};

remove function fn::query_tubi_to;
DEFINE FUNCTION fn::query_tubi_to($pe:record) {
    return select arrive from only $pe.owner->tubi_relate where leave=$pe limit 1;
};

remove function fn::query_tubi_from;
DEFINE FUNCTION fn::query_tubi_from($pe:record) {
    return select leave from only $pe.owner->tubi_relate where arrive=$pe limit 1;
};

remove function fn::query_bran_first_tubi;
DEFINE FUNCTION fn::query_bran_first_tubi($pe:record) {
    if $pe.noun != 'BRAN' {
        return none;
    };
    return tubi_relate:[$pe, 0].id;
};

remove function fn::prev_type;
DEFINE FUNCTION fn::prev_type($pe:record) {
    return fn::prev_pe($pe).noun;
};

remove function fn::next_type;
DEFINE FUNCTION fn::next_type($pe:record) {
    return fn::next_pe($pe).noun;
};


remove function fn::children;
DEFINE FUNCTION fn::children($pe:record) {
    return array::distinct(select value in from $pe<-pe_owner where record::exists(in.id) and !in.deleted);
};


// 获取ipara
remove function fn::get_ipara;
DEFINE FUNCTION fn::get_ipara($pe:record) {
    let $ispe = $pe.owner.refno.ISPE;
    return if $ispe == none {
        return [0];
    } else {
        return (select value (select value CATR.refno.PARA
                from only $ispe<-pe_owner<-pe<-pe_owner<-pe[? refno.TYPE = 'SELE'
                and $parent.owner.refno.TEMP >=refno.ANSW
                and $parent.owner.refno.TEMP <= refno.MAXA ]<-pe_owner<-pe.refno.*
                where $parent.owner.refno.HBOR >=ANSW and $parent.owner.refno.HBOR <= MAXA limit 1)
            from $pe)[0];
    };
};

//根据类型找到父节点
remove function fn::find_ancestor_type;
DEFINE FUNCTION fn::find_ancestor_type($pe: record, $t: string){
    (array::flatten(select value fn::ancestor(id) from $pe)[?$self.noun=$t])[0]
};

-- pe record id（"ref0_ref1"，历史行为 ['ref0_ref1', sesno]）→ RefU64 打包整数
-- （ref0<<32|ref1）。层级查询优化（docs/plans/2026-08-07-inst-relate-anc-u64-
-- hierarchy-query-plan.md）：anc/dbnum 走纯数字，与 record id 形制解耦。
-- ref0 实测两万级，乘积距 i64 上限很远；真越界由 int 溢出报错，不静默截断。
DEFINE FUNCTION OVERWRITE fn::refno_u64($r: record) {
    LET $raw = record::id($r);
    LET $id = if type::is::array($raw) { <string> array::at($raw, 0) } else { <string> $raw };
    LET $parts = string::split($id, '_');
    RETURN <int> $parts[0] * 4294967296 + <int> $parts[1];
};

-- 自身 → 顶层的祖先链（含自身），RefU64 打包整数数组。写 inst_relate/tubi_relate
-- 的 anc 字段用。走 pe.owner 记录链接（便宜的向上方向）；12 跳与 find_deep_children
-- 系列的层深一致（fn::ancestor 只有 9 跳，深层构件会够不到 SITE，这里不复用它）。
DEFINE FUNCTION OVERWRITE fn::anc_u64($r: record) {
    LET $chain = if $r.id == none { [] } else {
        array::complement(
            (SELECT VALUE [id,
                owner,
                owner.owner,
                owner.owner.owner,
                owner.owner.owner.owner,
                owner.owner.owner.owner.owner,
                owner.owner.owner.owner.owner.owner,
                owner.owner.owner.owner.owner.owner.owner,
                owner.owner.owner.owner.owner.owner.owner.owner,
                owner.owner.owner.owner.owner.owner.owner.owner.owner,
                owner.owner.owner.owner.owner.owner.owner.owner.owner.owner,
                owner.owner.owner.owner.owner.owner.owner.owner.owner.owner.owner,
                owner.owner.owner.owner.owner.owner.owner.owner.owner.owner.owner.owner]
             FROM ONLY $r LIMIT 1),
            [NONE, pe:0_0]
        )
    };
    RETURN array::map($chain, |$a| fn::refno_u64($a));
};

-- anc 尾部定位（层级查询优化 P3 的读侧便捷层）：anc = 自身→顶（含自身），
-- SITE/ZONE 是最上面的两个业务层级，从链尾取即为 O(1)，与元素深度无关。
-- 链尾有两种真实形态（生产以 1 为准）：
--   1. 顶是 ref1=0 的 WORL——全量入库刻意不存 WORL 行（database.rs 的
--      ignore_world_refno），元素 owner 仍指向它，两个生产者（Rust 的
--      resolve_inst_meta_on / 本文件的 fn::anc_u64）都把它收在链尾；
--   2. owner 是 pe:0_0 之类空哨兵、被 anc_u64 滤掉，链止于 SITE（防御形态）。
-- 判据与 Rust 解析器同源：打包值 ref1==0（% 2^32 == 0）即 WORL，据此定偏移。
-- 短链/空链由 array::at 越界返回 NONE；ZONE/SITE 自身的链按「含自身」语义
-- 返回自己（与退役的 fn::find_ancestor_type 口径一致）。
-- 反向圈行（某 ZONE 下全部实例）不要用它：那是 `anc CONTAINS $u64` 的索引查询。
DEFINE FUNCTION OVERWRITE fn::site_u64($anc: array) {
    if array::len($anc) == 0 { return NONE; };
    LET $off = if array::at($anc, -1) % 4294967296 == 0 { 1 } else { 0 };
    RETURN array::at($anc, -1 - $off);
};
DEFINE FUNCTION OVERWRITE fn::zone_u64($anc: array) {
    if array::len($anc) == 0 { return NONE; };
    LET $off = if array::at($anc, -1) % 4294967296 == 0 { 1 } else { 0 };
    RETURN array::at($anc, -2 - $off);
};

// 根据类型返回子节点
remove function fn::find_deep_children_type;
define function fn::find_deep_children_type($pe:record,$t:string) {
    let $children = array::flatten( object::values( (select
                      [id] as p0, <-pe_owner<-(? as p1)<-pe_owner<-(? as p2)<-pe_owner<-(? as p3)<-pe_owner<-(? as p4)<-pe_owner<-(? as p5)<-pe_owner<-(? as p6)<-pe_owner<-(? as p7)<-pe_owner<-(? as p8)<-pe_owner<-(? as p9)<-pe_owner<-(? as p10)<-pe_owner<-(? as p11)
                   from only $pe where record::exists(id))?:{{}} ) );

    return select value id from $children where id.noun == $t

};

// 返回类型集合的父节点
remove function fn::find_ancestor_types;
DEFINE FUNCTION fn::find_ancestor_types($pe: record, $t: array){
    let $r = (array::flatten(select value fn::ancestor(id) from $pe));
    return select value id from $r where noun in $t;
};

// 返回符合类型集合的子节点
remove function fn::find_deep_children_types;
define function fn::find_deep_children_types($pe:record,$t:array) {
    let $children = array::flatten( object::values( (select
                      [id] as p0, <-pe_owner<-(? as p1)<-pe_owner<-(? as p2)<-pe_owner<-(? as p3)<-pe_owner<-(? as p4)<-pe_owner<-(? as p5)<-pe_owner<-(? as p6)<-pe_owner<-(? as p7)<-pe_owner<-(? as p8)<-pe_owner<-(? as p9)<-pe_owner<-(? as p10)<-pe_owner<-(? as p11)
                   from only $pe where record::exists(id))?:{{}} ) );
    return select value id from $children where id.noun in $t

};

remove FUNCTION fn::find_door_from_wall;
DEFINE FUNCTION fn::find_door_from_wall($pe:array) {
    let $fitts = select value fn::find_deep_children_types(id, ["FIXING", "NBOX", "NCYL", "NXTR", "PFIT", "NPYR", "FITT", "SBFI"]) from $pe;
    let $uda_value = select id as refno,fn::default_name(id) as name,fn::find_ancestor_types(id,["STWALL", "GWALL", "WALL"])[0] as wall from array::flatten(select value id from $fitts) where string::contains(fn::get_uda_value(id,'/JGOBJBASE')?:'','门洞');
    return $uda_value;
};

remove function fn::get_mdb_dbnums;
DEFINE FUNCTION fn::get_mdb_dbnums($mdb: string, $db_type: int) {
        -- 同名 MDB 可能并存(设计库/目录库各一份 /ALL；目录库的设计 CURD 常为空)。
        -- 不再 `only ... limit 1`(会误选到 CURD 为空的目录 MDB → 返回空树)，改为
        -- flatten 所有同名 MDB 的设计 DBNO 列表：空列表自然消失，非空者按入库顺序
        -- (included_projects 主项目先入库)拼接，dbnum 冲突时下方 find_index 取首次出现即“首个为准”。
        let $dbnos = array::flatten(select value (select value DBNO from CURD.refno where STYP=$db_type) from MDB where NAME=$mdb);
        -- 只保留已解析(存在 WORL)的 dbnum，按 CURD 顺序返回(首个有数据者在前)。
        return (select value dbnum from (
            select REFNO.dbnum as dbnum, array::find_index($dbnos, REFNO.dbnum) as o
            from WORL
            where REFNO.dbnum in $dbnos
            order by o
        ));
};

remove function fn::get_flan_bolt_list;
define function fn::get_flan_bolt_list($pe:record) {
    if $pe.refno.SPRE == NONE { return NONE; };
    let $bltf = select value refno.SPRE.refno.CATR.refno.BLRF[0].name from $pe;
    if $bltf == NONE { return NONE; };
    let $bltf_split = array::last(string::split($bltf[0],'-')?:[]);
    let $blot_size = string::concat($bltf_split,'W')?:'';
    let $ancestor = fn::find_ancestor_type($pe.refno.SPRE?:pe:0_0,'SPEC').refno.BSPE;
    if $ancestor == NONE { return NONE; };
    let $spcos = fn::find_deep_children_type($ancestor,'SPCO');
    if $spcos == NONE { return NONE; };
    let $filter_spcos = select value id from $spcos where string::contains(name,$blot_size);
    if array::len($filter_spcos) == 0 { return NONE; };
    let $mat = select name , id.refno.DETR.refno.RTEX as rtext from $filter_spcos;
    return {
        'bltf':$bltf[0],
        'mat':$mat
    }
};
REMOVE FUNCTION fn::room_relate_of;
-- 取一个构件"最强"的那条房间归属边。
--
-- 数据模型允许一个件同时属于多间房（横跨两室的桶形件就是），而消费方只取一条。
-- 没有排序键时取到哪条由存储顺序决定：全量重建时碰巧稳定，一旦改成增量、边删了
-- 再写回去就会变，表现为材料表房间号无规律跳动且无任何日志（ADR-010 §5 / 缺陷 D7）。
--
-- 排序键由 save_room_relate 写入：inside_count 是元素 AABB 八顶点落在面板内的个数，
-- center_dist 是元素中心到面板中心的距离。第三级 room_num 保证全序，
-- 使旧数据（缺前两个字段）也有确定结果。
DEFINE FUNCTION fn::room_relate_of($pe: record) {
    return (select * from $pe<-room_relate
            order by inside_count desc, center_dist asc, room_num asc
            limit 1)[0];
};

REMOVE FUNCTION fn::room_num_of;
-- fn::room_relate_of 的房间号快捷方式。
DEFINE FUNCTION fn::room_num_of($pe: record) {
    return fn::room_relate_of($pe).room_num;
};
