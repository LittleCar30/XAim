#[test] fn values() { assert!(fixture::reader::read("a").is_ok()); assert!(fixture::writer::write("").is_err()); }
