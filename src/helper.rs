
#[inline]
pub fn qualified_table_name(table: &str) -> String{
    table.to_lowercase().replace("join", "joint").replace("loop","loop_")
}

#[inline]
pub fn qualified_column_name(column: &str) -> String{
    column.replace("desc", "desc_").replace("lock", "lock_").replace("char", "char_")
}