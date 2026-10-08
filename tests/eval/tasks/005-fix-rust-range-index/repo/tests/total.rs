use fixture::total;
#[test] fn empty() { assert_eq!(total(&[]), 0); }
#[test] fn numbers() { assert_eq!(total(&[1, 2, 3]), 6); assert_eq!(total(&[-2, 1]), -1); }
