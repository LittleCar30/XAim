pub fn range(start: i32, end: i32) -> Vec<i32> {
    if start >= end { return Vec::new(); }
    (start..end).collect()
}
