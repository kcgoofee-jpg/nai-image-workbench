use subtle::ConstantTimeEq;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthFail {
    Denied,
    Limited,
}

pub fn bearer(value: Option<&str>) -> &str {
    let Some(value) = value else {
        return "";
    };
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .unwrap_or(value)
        .trim()
}

pub fn keys_match(given: &str, expect: &str) -> bool {
    let given = given.as_bytes();
    let expect = expect.as_bytes();
    if given.len() != expect.len() {
        return false;
    }
    given.ct_eq(expect).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_strips_prefix() {
        assert_eq!(bearer(Some("Bearer secret-value")), "secret-value");
        assert_eq!(bearer(Some("secret-value")), "secret-value");
        assert_eq!(bearer(None), "");
    }

    #[test]
    fn match_is_exact() {
        assert!(keys_match("0123456789abcdef", "0123456789abcdef"));
        assert!(!keys_match("0123456789abcdef", "0123456789abcdee"));
        assert!(!keys_match("short", "0123456789abcdef"));
    }
}
