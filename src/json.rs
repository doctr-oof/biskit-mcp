/// `skip_serializing_if` predicate for flags whose absence already means false.
pub fn is_false(value: &bool) -> bool {
    !value
}
