pub fn read(value: &str) -> Result<String, String> { if value.is_empty() { Err("empty".into()) } else { Ok(value.into()) } }
