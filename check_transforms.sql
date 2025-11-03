-- 查询 21491_18946 的所有子孙节点的 inst_relate 数据
SELECT 
    fn::pe_to_refno(in.id) as refno,
    world_trans.d as world_trans,
    out.geo_hash as geo_hash,
    out.transform.d as inst_transform
FROM array::flatten(
    fn::collect_descendant_filter_ids(pe:⟨21491_18946⟩, [], [])->inst_relate
)
WHERE world_trans.d != none
ORDER BY fn::pe_to_refno(in.id);

