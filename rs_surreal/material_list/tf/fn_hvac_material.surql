// 材质
remove function fn::hvac_material;
DEFINE FUNCTION fn::hvac_material($pe:record) {
    let $mat_code_map = ['','SS', 'GS'	,'CS','ACS','ASS',	'AS', 'FP'];
    let $owner_name = $pe.owner.name?:'';
    let $owner_name_split = array::at(string::split($owner_name,'-'),3)?:'0';
    return
    select if $owner_name_split != '6' && string::is::numeric($owner_name_split){
        array::at($mat_code_map,<int>$owner_name_split)
    } else if fn::pre_or_next_b_type($this.id,'DAMP') { 'CS' } else {
        'FP'
    } as r from $pe
};