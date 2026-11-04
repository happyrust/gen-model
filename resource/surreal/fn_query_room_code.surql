remove function if exists fn::single_room_code;
// 目标查找节点的房间号。
// 跨多房构件（横跨两室的桶形件）会有多条 room_relate 边，这里取 ADR-010 §5 定义的
// 最强边（inside_count desc, center_dist asc, room_num asc），不再按存储序取首条。
// 无房间边时**先判空**：`fn::room_relate_of($pe).in` 在 NONE 上取字段会让整条语句以
// `CoerceTo { from: None, into: "record" }` 报错（2026-10-29 mem 引擎实测）。
DEFINE FUNCTION fn::single_room_code($pe:record) {
    let $res = fn::room_relate_of($pe);
    if $res == NONE {
        return none;
    };
    // $res.in 是面板 pe。缺面板行（数据残缺、或测试夹具没建面板）时，
    // `$res.in.owner` 会让整条语句以「Expected a record but found NONE」报错，
    // 所以面板与面板属主都先单独取出来判空，不在表达式里链式访问。
    let $panel = $res.in;
    if $panel == NONE {
        return none;
    };
    // 注意：`$panel.owner` 取到的是**记录 id**（`pe:…`），不是记录本身；
    // 记录 id 上取 `.noun` / `.name` 会回 NONE（mem 引擎实测，会拼出 "NONE-R144"）。
    // 显式 `select * from only` 把整行取出来再用。
    let $owner_id = $panel.owner;
    if $owner_id == NONE {
        return none;
    };
    let $owner = select * from only type::thing('pe', record::id($owner_id));
    if $owner == NONE {
        return none;
    };
    // 先取FRMW上:BPENCODEID的值，没有再从name上取
    let $room_code_uda = fn::get_uda_value($owner.id,'/BPENCODEID')?:'';
    if $room_code_uda != '' && $room_code_uda != 'unset' {
        return $room_code_uda;
    };
    // 前缀取「面板属主名字的第一段」（'/1RX-RM01-R144-VOLU' → '1RX'）。
    // SBFR 的属主是楼层面板，要再上一层取楼名；缺行时退回 SBFR 自己的名字。
    // 注意 mem 引擎的坑：`if <变量> != '' { … } else { … }` 这种**嵌套 IF 里的字符串
    // 比较**会判假（2026-10-29 实测，变量明明有值却走 else）。因此这里不写嵌套 IF，
    // 改成先把两个候选前缀都算出来，再用 `?:` 取第一个非空值。
    let $owner_name = $owner.name ?: '';
    let $owner_prefix = string::split(string::slice($owner_name, 1), '-')[0];
    let $upper_id = $owner.owner;
    let $upper_name = if $upper_id == NONE { '' } else { $upper_id.name ?: '' };
    let $upper_prefix = string::split(string::slice($upper_name, 1), '-')[0];
    let $prefix = if $owner.noun == 'SBFR' {
            ($upper_prefix ?: '') ?: $owner_prefix
        } else { $owner_prefix };
    if $prefix == '' {
        return none;
    };
    // 房间号归一：4 位直接用；5 位（如 R0144）去掉第二位，与「1RX-R144」同形。
    // 真库 167407 条边实测全是 4 位，5 位分支是给旧数据/其它项目留的（gen-model 的
    // hd 用例钉住了这条路）；其它长度回 NONE，不拼出畸形码。
    let $code = if string::len($res.room_num) == 4 {
        $res.room_num
    } else if string::len($res.room_num) == 5 {
        string::concat(string::slice($res.room_num, 0, 1), string::slice($res.room_num, 2))
    } else {
        none
    };
    if $code == none {
        return none;
    };
    return string::concat($prefix, '-', $code);
};

remove function if exists fn::room_vote_codes;
// 一组构件按房间投票，回房间码数组，名次靠前的在前；没有任何房间时回 `[]`。
// 每个构件一票，投给 `fn::room_relate_of` 的最强边。名次依次比：票数多、
// inside_count 合计大、center_dist 合计小、房间码字典序小。
// 先按（面板, room_num）分组，每组只拼一次房间码：逐件调 `fn::single_room_code`
// 会让跨房设备慢一倍以上（2026-11-04 8009 实测）。同一间房的几块面板拼出同一个码，
// 所以拼完再按房间码合一次票。
// `GROUP BY` 同一层写 `ORDER BY` 不生效，结果按分组键排（2026-11-04 8009 实测），
// 排序必须单独一条 SELECT。
// `$edge.in` 只在判过非空之后取：在 NONE 上取字段会让整条语句报错（mem 引擎实测）。
DEFINE FUNCTION fn::room_vote_codes($members: array) {
    let $edges = array::filter(
        array::map($members, |$m| {
            let $id = type::thing('pe', record::id($m));
            let $edge = fn::room_relate_of($id);
            return if $edge == NONE { NONE } else {
                {
                    member: $id,
                    panel: $edge.in,
                    room_num: $edge.room_num,
                    inside: $edge.inside_count ?? 0,
                    dist: $edge.center_dist ?? 0.0
                }
            };
        }),
        |$v| $v != NONE
    );
    if array::len($edges) == 0 {
        return [];
    };
    let $groups = (select panel, room_num, array::first(member) as member, count() as n,
        math::sum(inside) as inside, math::sum(dist) as dist
        from $edges group by panel, room_num);
    let $coded = array::filter(
        array::map($groups, |$g| {
            code: fn::single_room_code(type::thing('pe', record::id($g.member))),
            n: $g.n,
            inside: $g.inside,
            dist: $g.dist
        }),
        |$v| $v.code != NONE && $v.code != ''
    );
    if array::len($coded) == 0 {
        return [];
    };
    let $by_code = (select code, math::sum(n) as n, math::sum(inside) as inside,
        math::sum(dist) as dist from $coded group by code);
    let $ranked = (select * from $by_code order by n desc, inside desc, dist asc, code asc);
    return array::map($ranked, |$r| $r.code);
};

remove function if exists fn::room_code;
// 返回**数组**：消费方一律按 `fn::room_code($pe)[0]` 取首个房间码
// （core 的 `AiosDBMgr::get_room_code`、rs-server 的 `heat_dissipation.rs`）。
// 返回字符串会让 `[0]` 取到首字符，因此这里与 hh 版保持同一形状。
DEFINE FUNCTION fn::room_code($pe:record) {
    // 入口收敛：`$pe` 可能是记录 id，也可能是整行对象（mem 引擎在函数里读字段后会
    // 物化）。先取出 noun 与纯记录 id，后面**只用**这两个局部变量，不再碰 `$pe`。
    // 直接把整行对象传给 `fn::single_room_code` 会让它内部的 `record::id($pe)` 以
    // 「Expected a record but found { id: pe:…, … }」报错（2026-10-29 mem 引擎实测）。
    let $noun = $pe.noun;
    let $pe_id = type::thing('pe', record::id($pe));
    // 优先使用接口保存到 room_data 表中的房间编码。
    // room_data 与 pe 使用相同的 refno 作为记录主键，例如：
    // pe:24383_67869 -> room_data:24383_67869。
    // 记录不存在时 type::thing 回 NONE，直接 `$saved_room_data.type` 会在 NONE 上取字段
    // 并以「Expected a record but found NONE」整条报错（2026-10-29 mem 引擎实测），
    // 所以先取记录本身再判空。
    let $room_data_id = type::thing('room_data', record::id($pe_id));
    let $saved_room_data = select * from only $room_data_id;
    let $saved_type = if $saved_room_data == NONE { NONE } else { $saved_room_data.type };
    let $saved_code = if $saved_room_data == NONE { NONE } else { $saved_room_data.room };
    let $query_type = string::uppercase($saved_type ?: $noun ?: '');

    // BRAN 返回其所有直接子节点保存的房间编码。
    if $query_type == 'BRAN' {
        let $child_room_codes = array::distinct(array::complement(
            (select value type::thing('room_data', record::id(id)).room
                from array::flatten($pe_id<-pe_owner.in.id)),
            [NONE, '']
        ));
        if array::len($child_room_codes) > 0 {
            return $child_room_codes;
        };
    };

    let $saved_room_code = $saved_code;
    if $saved_room_code != none && $saved_room_code != '' {
        return [$saved_room_code];
    };

    // 注意：这里**不能**写 `return if … { return … } else { … }`——把 `return` 嵌在
    // `return` 的表达式里，mem 引擎会把分支里的 `return` 当成表达式值，`$pe` 退化成
    // 整行对象后 `record::id($pe)` 报「Expected a record but found { … }」。
    // 改成平铺的 IF 语句，每支自己 return（2026-10-29 实测）。
    // 设备、支吊架、区域与 BRAN 都由子孙件投票（`fn::room_vote_codes`），不按存储序取首个。
    // EQUI可能有两层
    if ($noun == 'EQUI' || $noun == 'STRU' || $noun == 'REST') {
        let $codes = fn::room_vote_codes(array::flatten([
            $pe_id<-pe_owner.in.id,
            array::flatten($pe_id<-pe_owner.in<-pe_owner.in.id)
        ]));
        if array::len($codes) == 0 {
            return none;
        };
        return [$codes[0]];
    };
    // BRAN 回整张名次表，首个是得票最多的房间。
    if $noun == 'BRAN' {
        return fn::room_vote_codes(array::flatten($pe_id<-pe_owner.in.id));
    };
    if $noun == 'SUPPO' || $noun == 'ZONE' {
        let $codes = fn::room_vote_codes(array::flatten(array::flatten(array::flatten(
            $pe_id<-pe_owner.in<-pe_owner.in<-pe_owner.in.id
        ))));
        if array::len($codes) == 0 {
            return none;
        };
        return [$codes[0]];
    };
    // 其余构件（PANE / TUBI / 普通板件）看自己。
    // 再次收敛：`$pe_id` 被读过字段后会退化成整行对象，直接传给 `single_room_code`
    // 会以「Expected a record but found { … }」报错（mem 引擎实测）。
    let $self_code = fn::single_room_code(type::thing('pe', record::id($pe_id)));
    if $self_code == NONE || $self_code == '' {
        return none;
    };
    return [$self_code];
};

remove function fn::get_room_code_from_name;
define function fn::get_room_code_from_name($pe:record) {
    let $name = $pe.name;
    if $name == none { return none };
    // 分割并判断是否是房间号
    let $room = string::split($name,'-')[1];
    if $room == NONE || !string::matches($room,"^[A-Z][0-9]{3}$") { return NONE; };
    return $room;
    // 找到对应的frmw
    let $frmw = (select value NAME from FRMW where NAME != NONE && string::ends_with(NAME,string::concat('-R-',$room)))[0];
    if $frmw == NONE { return NONE; };
    let $frmw_split = string::split($frmw,'-');
    // 直接用数组索引，方便判断是否是房间，不是房间就是NONE
    let $room_code = string::concat(string::slice($frmw_split[0]?:'/',1),'-',$frmw_split[4]?:'');
    return if $room_code == '-' { return NONE; } else { return $room_code; };
};
