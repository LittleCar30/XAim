pub fn detail(cents: i64) -> String { format!("${}.{:02}", cents / 100, cents % 100) }
