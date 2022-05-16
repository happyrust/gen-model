
//重点，经常使用的部分，单独成表
//不重要的部分用bincode 保存起来
pub struct PdmsElement{
    pub refno: String,   //暂时用string，看看性能对比
    pub owner: String,
    pub name: String,
    pub type_name: String,
    pub foregin_data_id: String,  //跳到对应的表单里去寻找, 这里相当于是个索引数据
    //数据结构太多了，不适合创建那么多struct，ORM可以暂时不使用，经常处理的数据结构，可以用struct来处理
    // pub struct_id: String,     //结构的数据, 指向 refno，相当于外键
    // pub piping_id: String,     //管道的数据
    // pub equip_id: String,      //设备的数据
    // pub cata_id: String,       //元件库数据
    // pub primitive_id: String,  //基本体的数据
}

//不同的类型需要跳表，创建表单

pub struct PdmsStrut{
    pub refno: String,
    pub spref: String,  //作为foreign key
    pub positions: Vec<u8>, //存储不同position的数据，
}


//直接全写sql可行吗，要不要这样拆分，创建table