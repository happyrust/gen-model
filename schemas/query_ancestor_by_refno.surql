--从节点出发找到根节点，默认层级深度为 9
let $a = SELECT ->pe_owner->(? AS f1)->pe_owner->(? AS f2)->pe_owner->(? AS f3)->pe_owner->(? AS f4)->pe_owner->(? AS f5)->pe_owner->(? AS f6)->pe_owner->(? AS f7)->pe_owner->(? AS f8)->pe_owner->(? AS f9)  from only (type::thing("pe", $refno));
array::flatten([$a['f1'], $a['f2'], $a['f3'], $a['f4'], $a['f5'], $a['f6'], $a['f7'], $a['f8'], $a['f9']]);