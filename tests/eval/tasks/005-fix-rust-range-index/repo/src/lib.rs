pub fn total(items: &[i32]) -> i32 {
    let mut result = 0;
    for index in 0..=items.len() {
        result += items[index];
    }
    result
}
