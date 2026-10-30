// 房间面板的确定性枚举（room_height / room_top_height 的公共前半段）。
//
// 为什么单独抽一个函数（2026-10-30 修）：
// 原来 room_height 与 room_top_height 各自用 `array::distinct(...)` 收集面板标高，
// 而 `array::distinct` **不排序**——它只按元素第一次出现的顺序去重。多房间构件
// 因此拿到的数组顺序取决于遍历/存储顺序，消费方 `fn::room_height(id)[0]`
// （core `aios_mgr.rs:91` 的 `query_own_room_panel_elevations`）取到哪个房间全凭运气。
//
// 真库实测（2026-10-30）：`pe:15516_104` 回 `[-280.0, -20.0]`，而同样落在这两间房的
// `pe:15516_15` / `pe:15516_4` 回 `[-20.0, -280.0]`。净高 = top[0] - bottom[0] 时，
// 若 bottom 与 top 两个数组的顺序还不同步（它们分别独立 distinct），会拼出
// `2705 - (-280) = 2985` 这种根本不存在的净高。这与 ADR-010 §5 修 room_relate_of
// 治的是同一个病（缺陷 D7：顺序依赖存储序、无日志），只是那一次只修了「取哪条边」，
// 没修「多条边按什么顺序排」。
//
// 排序键选**面板记录 id**而不是 room_num 或标高：
// - 标高不能当键：同一标高可能属于不同房间，键会撞；且 bottom 与 top 的排序若按各自
//   数值排，两边顺序会不一致，配对就散了。
// - room_num 能当键，但 `array::sort` 的闭包在 mem 引擎里取不到外层 `let` 变量
//   （2026-10-30 实测 `array::filter($edges, |$e| $e.room_num == $rn)` 恒空数组），
//   没法先 sort 再回查面板。
// - 面板记录 id 天然唯一，`array::sort` 对 record 数组可直接排序（实测
//   `array::sort([pe:2_1, pe:1_5, pe:10_3])` → `[pe:10_3, pe:1_5, pe:2_1]`），
//   一次排序同时定死 bottom 与 top 的顺序，两者必然配对。
//
// 契约：返回面板 pe 的记录 id 数组，已去重、已按记录 id 升序。
// 无房间归属时回 `[]`（不是 NONE），与旧写法形状一致。
REMOVE FUNCTION fn::room_panels_of;
DEFINE FUNCTION fn::room_panels_of($deep: array) {
    let $panels = array::filter(
        array::map($deep, |$c| fn::room_relate_of($c).in),
        |$v| $v != NONE
    );
    return array::sort(array::distinct($panels));
};

// 底标高
REMOVE FUNCTION fn::room_height;
DEFINE FUNCTION fn::room_height($pe:record) {
    let $noun = $pe.refno.TYPE;
    // 面板几何读法（2026-10-29 修）：room_relate.in 是面板 pe，几何在**普通表**
    // inst_relate:⟨ref0_ref1⟩ 上（不是从 pe 出去的边），所以旧写法
    // `<-room_relate.in.id->inst_relate.aabb.d.mins[2]` 恒为 NONE（真库实测 [null]）。
    // 现在先取面板（跨多房按 ADR-010 §5 最强边，多面板按记录 id 排序），再按键读 aabb_d。
    // array::filter 滤掉 NONE：消费方（core query_own_room_panel_elevations）按
    // Vec<f32> 解码，混入 NONE 会让整条解码失败并静默落 0.0。
    return
    // EQUI可能有两层
    if $noun == 'EQUI' || $noun == 'STRU' || $noun == 'REST' {
        array::map(
            fn::room_panels_of(array::flatten([
                $pe<-pe_owner.in.id,
                array::flatten($pe<-pe_owner.in<-pe_owner.in.id)
            ])),
            |$p| type::thing('inst_relate', record::id($p)).aabb_d.mins[2]
        )
    } else if $noun == 'BRAN' {
        array::map(
            fn::room_panels_of(array::flatten($pe<-pe_owner.in.id)),
            |$p| type::thing('inst_relate', record::id($p)).aabb_d.mins[2]
        )
    } else  {
        array::map(
            fn::room_panels_of([$pe]),
            |$p| type::thing('inst_relate', record::id($p)).aabb_d.mins[2]
        )
    }
};

// 顶标高
// 与 room_height **共用** fn::room_panels_of，所以同一构件的 bottom[i] 与 top[i]
// 必然来自同一块面板；两处分别 distinct 的旧写法不保证这一点。
REMOVE FUNCTION fn::room_top_height;
DEFINE FUNCTION fn::room_top_height($pe:record) {
    let $noun = $pe.refno.TYPE;
    return
    // EQUI可能有两层
    if $noun == 'EQUI' || $noun == 'STRU' || $noun == 'REST' {
        array::map(
            fn::room_panels_of(array::flatten([
                $pe<-pe_owner.in.id,
                array::flatten($pe<-pe_owner.in<-pe_owner.in.id)
            ])),
            |$p| type::thing('inst_relate', record::id($p)).aabb_d.maxs[2]
        )
    } else if $noun == 'BRAN' {
        array::map(
            fn::room_panels_of(array::flatten($pe<-pe_owner.in.id)),
            |$p| type::thing('inst_relate', record::id($p)).aabb_d.maxs[2]
        )
    } else  {
        array::map(
            fn::room_panels_of([$pe]),
            |$p| type::thing('inst_relate', record::id($p)).aabb_d.maxs[2]
        )
    }
};


// return fn::room_height(pe:24384_24804)[0];
