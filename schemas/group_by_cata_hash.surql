
let $a = array::flatten(select value <-pe_owner.in from $refnos);
select cata_hash as k, array::group(id) as v from $a group by k;
