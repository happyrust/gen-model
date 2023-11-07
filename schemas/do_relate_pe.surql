LET $pes = (SELECT id, owner FROM pe);

DEFINE INDEX unique_pe_owner
    ON TABLE pe_owner
    COLUMNS in, out UNIQUE;

for $p in $pes{
    //update record link
    (update (<record> $p['id']) set owner=(type::thing("pe", $p['owner'])));
};

//for $p in $pes{
    //create relate
//    (relate ($p['id'])->pe_owner->(type::thing("pe", $p['owner'])));
//};