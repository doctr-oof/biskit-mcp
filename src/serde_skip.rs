/// `skip_serializing_if` predicate for flags whose absence already means false.
pub fn is_false(value: &bool) -> bool {
    !value
}

/// `skip_serializing_if` predicate for counts whose absence already means none.
pub fn is_zero(value: &usize) -> bool {
    *value == 0
}
