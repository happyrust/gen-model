use nom::character::complete::tab;


//统一使用大写
#[inline]
pub fn qualified_table_name(table: &str) -> String{
    table.replace("JOIN", "JOINT").replace("LOOP","LOOP_")
}

#[inline]
pub fn qualified_column_name(column: &str) -> String{
    column.replace("DESC", "DESC_").replace("LOCK", "LOCK_").replace("CHAR", "CHAR_")
}