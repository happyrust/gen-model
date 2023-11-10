LET $pes = (SELECT id, owner, noun, refno FROM pe where !type::is::record(owner));

DEFINE INDEX unique_pe_owner
    ON TABLE pe_owner
    COLUMNS in, out UNIQUE;

for $p in $pes{
    //update owner record link
    (update (<record> $p['id']) set owner=(type::thing("pe", $p['owner'])));
};


for $p in $pes{
    //update refno record link
    (update (<record> $p['id']) set refno=(type::thing($p["noun"], $p['refno'])));
};

//for $p in $pes{
    //create relate
//    (relate ($p['id'])->pe_owner->(type::thing("pe", $p['owner'])));
//};