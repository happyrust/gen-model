thread 'tokio-runtime-worker' (2574546) panicked at /Volumes/DPC/work/plant-code/gen-model/src/fast_model/gen_model.rs:146:18:







布尔运算use_cate模型数据失败: SQL: 







        select







                in as refno,







                in.sesno as sesno,







                in.noun as noun,







                world_trans.d as wt,







                aabb.d as aabb,







                (select value [out, trans.d] from out->geo_relate where geo_type in ["Compound", "Pos"] and trans.d != NONE ) as ts,







                (select value [in, world_trans.d,







                    (select out as id, geo_type, trans.d as trans, out.aabb.d as aabb







                    from array::flatten(out->geo_relate) where trans.d != NONE and ( geo_type=="Neg" or (geo_type=="CataCrossNeg"







                        and geom_refno in (select value ngmr from pe:21895_66645<-ngmr_relate) ) ))]







                        from array::flatten([array::flatten(in<-neg_relate.in->inst_relate), array::flatten(in<-ngmr_relate.in->inst_relate)]) where world_trans.d!=none







                ) as neg_ts







             from inst_relate:21895_66645 where in.id != none and !bad_bool and ((in<-neg_relate)[0] != none or in<-ngmr_relate[0] != none) and aabb.d != NONE







         @ /Volumes/DPC/work/plant-code/rs-core/src/rs_surreal/query_ext.rs:65:24















Caused by:







    Internal error: Failed to deserialize field 'neg_ts' on type 'ManiGeoTransQuery': Failed to convert to array<[record<pe>, object, array<{aabb: none | object, geo_type: string, id: record, para_type: string, trans: object}>]>: Failed to convert to [record<pe>, object, array<{aabb: none | object, geo_type: string, id: record, para_type: string, trans: object}>]: Failed to convert to array<{aabb: none | object, geo_type: string, id: record, para_type: string, trans: object}>: Failed to deserialize field 'para_type' on type 'NegInfo': Expected string, got none