use fixture::port;
#[test] fn valid() { assert_eq!(port("0"), 0); assert_eq!(port("443"), 443); assert_eq!(port("65535"), 65535); }
#[test] fn invalid() { for input in ["", "nope", "-1", "65536"] { assert_eq!(port(input), 8080); } }
