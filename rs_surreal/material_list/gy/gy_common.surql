//定义一些工艺专业的公共函数

remove function fn::bran_tubi_len;
//首先查询所有 tubi 的长度 
define function fn::bran_tubi_len($pe:record) {
    return if $pe.noun != 'BRAN' {
        return 0;
    } else {
        return math::sum(select value world_trans.d.scale[2] from $pe->tubi_relate);
    }
};

remove function fn::elbo_len;
//计算管道元件的长度 
define function fn::elbo_len($pe:record) {
    return if $pe.noun != 'ELBO' && $pe.noun != 'BEND' {
        return 0;
    } else {
        return (select value (out.param.PrimCTorus.rins + 1.0)/2.0 * math::deg2rad(out.param.PrimCTorus.angle) * trans.d.scale[0]
            from ($pe->inst_relate.out->geo_relate)[0])[0]?:0.0;
    }
};

remove function fn::tee_len;
define function fn::tee_len($pe:record) {
    return if $pe.noun != 'TEE' {
        0
    } else {
        return math::sum(select value trans.d.scale[2] from ($pe->inst_relate.out->geo_relate)[0] where out=inst_geo:⟨2⟩);
    }
};

remove function fn::bran_comp_len;
//计算管道元件的长度
define function fn::bran_comp_len($pe:record) {
    return if $pe.owner.noun != 'BRAN' {
        return 0.0;
    } else {
        return if $pe.noun == 'ELBO' || $pe.noun == 'BEND' {
            return fn::elbo_len($pe);
        } else if $pe.noun == 'TEE' {
            return fn::tee_len($pe);
        } else {
            let $ptset = ($pe->inst_relate.out.ptset)[0];
            let $l = <string> $pe.refno.LEAV;
            let $a = <string> $pe.refno.ARRI;
            let $start_pt = $ptset[$l].pt;
            let $end_pt = $ptset[$a].pt;
            return if $start_pt == none || $end_pt == none {
                0.0;
            } else {
                vector::distance::euclidean($start_pt, $end_pt);
            }
        };
    }
};

remove function fn::bran_len;
define function fn::bran_len($pe:record) {
   //首先查询所有 tubi 的长度 
   let $tubi_len = fn::bran_tubi_len($pe);
   let $len = math::sum(select value fn::bran_comp_len(in) from $pe<-pe_owner);
   return $tubi_len + $len;
};
