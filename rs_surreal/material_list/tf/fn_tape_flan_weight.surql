// 法兰重量
remove function fn::tape_flan_weight;
DEFINE FUNCTION fn::tape_flan_weight($pe:record) {
    let $desp = $pe.refno.DESP;
    let $flan_type = if array::first($desp)[57] == 531717 { 'FJ' } else if array::first($desp)[57] == 531501 { 'FB' } else { '' };
    let $flan_length = string::split(fn::tape_flan_length($pe),',');
    return

    select if ($flan_type == 'FJ' && array::first($desp)[67] == 25) {
    string::concat(<string>(<float>$flan_length[0] * 1.124),',',<string>(<float>$flan_length[1] * 1.124))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 303) {
    string::concat(<string>(<float>$flan_length[0] * 1.373) ,',',<string>(<float>$flan_length[1] * 1.373))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 30) {
    string::concat(<string>(<float>$flan_length[0] * 1.786),',', <string>(<float>$flan_length[1] * 1.786))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 35) {
    string::concat(<string>(<float>$flan_length[0] * 1.656),',', <string>(<float>$flan_length[1] * 1.656))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 40) {
    string::concat(<string>(<float>$flan_length[0] * 2.422),',', <string>(<float>$flan_length[1] * 2.422))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 45) {
    string::concat(<string>(<float>$flan_length[0] * 2.736 ),',', <string>(<float>$flan_length[1] * 2.736 ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 50) {
    string::concat(<string>(<float>$flan_length[0] * 3.77 ),',', <string>(<float>$flan_length[1] * 3.77 ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 60) {
    string::concat(<string>(<float>$flan_length[0] * 5.369 ),',', <string>(<float>$flan_length[1] * 5.369 ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 608) {
    string::concat(<string>(<float>$flan_length[0] * 7.081 ),',', <string>(<float>$flan_length[1] * 7.081 ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 6550) {
    string::concat(<string>(<float>$flan_length[0] * 5.134  ),',', <string>(<float>$flan_length[1] * 5.134  ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 65) {
    string::concat(<string>(<float>$flan_length[0] * 5.3694   ),',', <string>(<float>$flan_length[1] * 5.3694   ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 63) {
    string::concat(<string>(<float>$flan_length[0] * 5.721    ),',', <string>(<float>$flan_length[1] * 5.721    ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 70) {
    string::concat(<string>(<float>$flan_length[0] * 7.398    ),',', <string>(<float>$flan_length[1] * 7.398    ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 7550) {
    string::concat(<string>(<float>$flan_length[0] * 5.699    ),',', <string>(<float>$flan_length[1] * 5.699    ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 75) {
   string::concat(<string>(<float>$flan_length[0] * 7.976),',', <string>(<float>$flan_length[1] * 7.976    ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 8060) {
   string::concat(<string>(<float>$flan_length[0] * 6.311),',', <string>(<float>$flan_length[1] * 6.311    ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 80) {
   string::concat(<string>(<float>$flan_length[0] * 9.658),',', <string>(<float>$flan_length[1] * 9.658    ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 8010) {
   string::concat(<string>(<float>$flan_length[0] * 11.874),',', <string>(<float>$flan_length[1] * 11.874    ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 90) {
   string::concat(<string>(<float>$flan_length[0] * 5.661),',', <string>(<float>$flan_length[1] * 5.661    ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 9056) {
   string::concat(<string>(<float>$flan_length[0] * 5.661),',', <string>(<float>$flan_length[1] * 5.661    ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 10065) {
   string::concat(<string>(<float>$flan_length[0] * 9.8596),',', <string>(<float>$flan_length[1] * 9.8596    ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 10080) {
   string::concat(<string>(<float>$flan_length[0] * 10.946),',', <string>(<float>$flan_length[1] * 10.946    ))
} else if ($flan_type == 'FJ' && array::first($desp)[67] == 100) {
   string::concat(<string>(<float>$flan_length[0] * 12.276),',', <string>(<float>$flan_length[1] * 12.276    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 253) {
   string::concat(<string>(<float>$flan_length[0] * 0.59),',', <string>(<float>$flan_length[1] * 0.59    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 254) {
   string::concat(<string>(<float>$flan_length[0] * 0.78),',', <string>(<float>$flan_length[1] * 0.78    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 304) {
   string::concat(<string>(<float>$flan_length[0] * 0.94),',', <string>(<float>$flan_length[1] * 0.94    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 305) {
   string::concat(<string>(<float>$flan_length[0] * 1.18),',', <string>(<float>$flan_length[1] * 1.18    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 354) {
   string::concat(<string>(<float>$flan_length[0] * 1.10),',', <string>(<float>$flan_length[1] * 1.10    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 404) {
   string::concat(<string>(<float>$flan_length[0] * 1.26),',', <string>(<float>$flan_length[1] * 1.26    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 405) {
   string::concat(<string>(<float>$flan_length[0] * 1.57),',', <string>(<float>$flan_length[1] * 1.57    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 505) {
   string::concat(<string>(<float>$flan_length[0] * 1.96),',', <string>(<float>$flan_length[1] * 1.96    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 606) {
   string::concat(<string>(<float>$flan_length[0] * 2.826),',', <string>(<float>$flan_length[1] * 2.826    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 608) {
   string::concat(<string>(<float>$flan_length[0] * 3.77),',', <string>(<float>$flan_length[1] * 3.77    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 6010) {
   string::concat(<string>(<float>$flan_length[0] * 4.71),',', <string>(<float>$flan_length[1] * 4.71    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 808) {
   string::concat(<string>(<float>$flan_length[0] * 5.02),',', <string>(<float>$flan_length[1] * 5.02    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 8010) {
   string::concat(<string>(<float>$flan_length[0] * 6.28),',', <string>(<float>$flan_length[1] * 6.28    ))
} else if ($flan_type == 'FB' && array::first($desp)[67] == 8012) {
   string::concat(<string>(<float>$flan_length[0] * 7.54),',', <string>(<float>$flan_length[1] * 7.54    ))
} else {
    '0,0'
} as weight from $pe
};

return fn::tape_flan_weight(pe:24381_76894);