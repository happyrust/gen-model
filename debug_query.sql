SELECT in as refno, in.sesno as sesno, in.noun as noun, world_trans.d as wt, aabb.d as aabb FROM inst_relate:30101_14380 WHERE in.id != none and !bad_bool and aabb.d != NONE;

-- 查询负实体关系
SELECT * FROM array::flatten(pe:30101_14380<-neg_relate.in<-inst_relate);

SELECT * FROM array::flatten(pe:30101_14380<-ngmr_relate.in->inst_relate);

-- 检查是否存在正实体几何
SELECT out as id, trans.d as trans FROM inst_relate:30101_14380.out->geo_relate WHERE geo_type in ["Compound", "Pos"] and trans.d != NONE;