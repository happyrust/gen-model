insert into tf_strt_flan_size [
{
    "id": 1,
    "fl_type": "FJ",
    "fl_size": 25,
    "size": "L25x3"
},
{
    "id": 2,
    "fl_type": "FJ",
    "fl_size": 303,
    "size": "L30x3"
},
{
    "id": 3,
    "fl_type": "FJ",
    "fl_size": 30,
    "size": "L30x4"
},
{
    "id": 4,
    "fl_type": "FJ",
    "fl_size": 35,
    "size": "L35x4"
},
{
    "id": 5,
    "fl_type": "FJ",
    "fl_size": 40,
    "size": "L40x4"
},
{
    "id": 6,
    "fl_type": "FJ",
    "fl_size": 45,
    "size": "L45x4"
},
{
    "id": 7,
    "fl_type": "FJ",
    "fl_size": 50,
    "size": "L50x5"
},
{
    "id": 8,
    "fl_type": "FJ",
    "fl_size": 60,
    "size": "L60x6"
},
{
    "id": 9,
    "fl_type": "FJ",
    "fl_size": 608,
    "size": "L60x8"
},
{
    "id": 10,
    "fl_type": "FJ",
    "fl_size": 6550,
    "size": "L65x50x6"
},
{
    "id": 11,
    "fl_type": "FJ",
    "fl_size": 65,
    "size": "L65x6"
},
{
    "id": 12,
    "fl_type": "FJ",
    "fl_size": 63,
    "size": "L63x6"
},
{
    "id": 13,
    "fl_type": "FJ",
    "fl_size": 70,
    "size": "L70x7"
},
{
    "id": 14,
    "fl_type": "FJ",
    "fl_size": 7550,
    "size": "L75x50x6"
},
{
    "id": 15,
    "fl_type": "FJ",
    "fl_size": 75,
    "size": "L75x7"
},
{
    "id": 16,
    "fl_type": "FJ",
    "fl_size": 8060,
    "size": "L80x60x6"
},
{
    "id": 17,
    "fl_type": "FJ",
    "fl_size": 80,
    "size": "L80x8"
},
{
    "id": 18,
    "fl_type": "FJ",
    "fl_size": 8010,
    "size": "L80x10"
},
{
    "id": 19,
    "fl_type": "FJ",
    "fl_size": 90,
    "size": "L90x9"
},
{
    "id": 20,
    "fl_type": "FJ",
    "fl_size": 9056,
    "size": "L90x56x5"
},
{
    "id": 21,
    "fl_type": "FJ",
    "fl_size": 10065,
    "size": "L100x65x8"
},
{
    "id": 22,
    "fl_type": "FJ",
    "fl_size": 10080,
    "size": "L100x80x8"
},
{
    "id": 23,
    "fl_type": "FJ",
    "fl_size": 100,
    "size": "L100x8"
},
{
    "id": 24,
    "fl_type": "FB",
    "fl_size": 253,
    "size": "FB25x3"
},
{
    "id": 25,
    "fl_type": "FB",
    "fl_size": 254,
    "size": "FB25x4"
},
{
    "id": 26,
    "fl_type": "FB",
    "fl_size": 304,
    "size": "FB30x4"
},{
    "id": 27,
    "fl_type": "FB",
    "fl_size": 305,
    "size": "FB30x5"
},
{
    "id": 28,
    "fl_type": "FB",
    "fl_size": 354,
    "size": "FB35x4"
},
{
    "id": 29,
    "fl_type": "FB",
    "fl_size": 404,
    "size": "FB40x4"
},
{
    "id": 30,
    "fl_type": "FB",
    "fl_size": 405,
    "size": "FB40x5"
},
{
    "id": 31,
    "fl_type": "FB",
    "fl_size": 505,
    "size": "FB50x5"
},
{
    "id": 32,
    "fl_type": "FB",
    "fl_size": 606,
    "size": "FB60x6"
},
{
    "id": 33,
    "fl_type": "FB",
    "fl_size": 608,
    "size": "FB60x8"
},
{
    "id": 34,
    "fl_type": "FB",
    "fl_size": 6010,
    "pre_b_damp": true,
    "bran_b_6": true,
    "size": "FB60x10"
},
{
    "id": 35,
    "fl_type": "FB",
    "fl_size": 808,
    "size": "FB80x8"
},
{
    "id": 36,
    "fl_type": "FB",
    "fl_size": 8010,
    "size": "FB80x10"
},
{
    "id": 37,
    "fl_type": "FB",
    "fl_size": 8012,
    "size": "FB80x12"
}
]


// (fn::b_pre_type($pe,'DAMP') || fn::b_pre_type($pe,'MESH')) && (fn::b_next_type($pe,'DAMP') || fn::b_next_type($pe,'MESH')) && !$bran_b_6