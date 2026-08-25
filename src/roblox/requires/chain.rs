const CHILD_BY_NAME_METHODS: [&str; 3] = ["GetService", "WaitForChild", "FindFirstChild"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Step {
    Child(String),
    Parent,
}

pub(super) struct Chain {
    pub(super) head: String,
    pub(super) steps: Vec<Step>,
}

pub(super) fn parse_chain(expression: &str) -> Result<Chain, String> {
    let chars: Vec<char> = expression.chars().collect();
    let mut index = 0;

    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    let (head, next) = read_identifier(&chars, index);
    if head.is_empty() {
        return Err("not an instance expression".to_string());
    }
    index = next;

    let mut steps = Vec::new();
    loop {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        if index >= chars.len() {
            return Ok(Chain { head, steps });
        }

        match chars[index] {
            '.' => {
                let (name, next) = read_identifier(&chars, index + 1);
                if name.is_empty() {
                    return Err("not an instance expression".to_string());
                }
                index = next;
                steps.push(match name.as_str() {
                    "Parent" => Step::Parent,
                    _ => Step::Child(name),
                });
            }
            ':' => {
                let (method, next) = read_identifier(&chars, index + 1);
                let (argument, next) = read_call_argument(&chars, next);
                index = next;
                match argument {
                    Some(name) if CHILD_BY_NAME_METHODS.contains(&method.as_str()) => {
                        steps.push(Step::Child(name))
                    }
                    _ => return Err(format!("cannot resolve :{method}(...) statically")),
                }
            }
            '[' => {
                let (argument, next) = read_bracket_argument(&chars, index);
                index = next;
                match argument {
                    Some(name) => steps.push(Step::Child(name)),
                    None => return Err("indexed by a value, not a literal name".to_string()),
                }
            }
            '(' => return Err("the require argument is a call result".to_string()),
            _ => return Err("not an instance expression".to_string()),
        }
    }
}

fn read_identifier(chars: &[char], from: usize) -> (String, usize) {
    let mut index = from;
    let mut identifier = String::new();
    while index < chars.len() && (chars[index].is_alphanumeric() || chars[index] == '_') {
        identifier.push(chars[index]);
        index += 1;
    }
    (identifier, index)
}

fn read_call_argument(chars: &[char], from: usize) -> (Option<String>, usize) {
    let mut index = from;
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    if index >= chars.len() || chars[index] != '(' {
        return (None, index);
    }

    let mut depth = 0usize;
    let mut literal = None;
    let mut only_literal = true;
    while index < chars.len() {
        match chars[index] {
            '(' => {
                depth += 1;
                index += 1;
            }
            ')' => {
                depth -= 1;
                index += 1;
                if depth == 0 {
                    return (literal.filter(|_| only_literal), index);
                }
            }
            quote @ ('"' | '\'') => {
                let (text, next) = read_string(chars, index, quote);
                if literal.is_none() {
                    literal = Some(text);
                }
                index = next;
            }
            character if character.is_whitespace() || character == ',' => index += 1,
            _ => {
                if literal.is_none() {
                    only_literal = false;
                }
                index += 1;
            }
        }
    }
    (None, index)
}

fn read_bracket_argument(chars: &[char], from: usize) -> (Option<String>, usize) {
    let mut index = from + 1;
    let mut literal = None;
    let mut only_literal = true;
    while index < chars.len() {
        match chars[index] {
            ']' => return (literal.filter(|_| only_literal), index + 1),
            quote @ ('"' | '\'') => {
                let (text, next) = read_string(chars, index, quote);
                if literal.is_none() {
                    literal = Some(text);
                }
                index = next;
            }
            character if character.is_whitespace() => index += 1,
            _ => {
                only_literal = false;
                index += 1;
            }
        }
    }
    (None, index)
}

pub(super) fn read_string(chars: &[char], from: usize, quote: char) -> (String, usize) {
    let mut index = from + 1;
    let mut text = String::new();
    while index < chars.len() {
        match chars[index] {
            '\\' if index + 1 < chars.len() => {
                text.push(chars[index + 1]);
                index += 2;
            }
            character if character == quote => return (text, index + 1),
            character => {
                text.push(character);
                index += 1;
            }
        }
    }
    (text, index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chains_parse_into_the_traversal_they_describe() {
        let chain = parse_chain("script.Parent.Parent.Shared.Combat").unwrap();
        assert_eq!(chain.head, "script");
        assert_eq!(
            chain.steps,
            vec![
                Step::Parent,
                Step::Parent,
                Step::Child("Shared".to_string()),
                Step::Child("Combat".to_string()),
            ]
        );

        let waited = parse_chain("ReplicatedStorage:WaitForChild(\"Shared\").Combat").unwrap();
        assert_eq!(waited.head, "ReplicatedStorage");
        assert_eq!(
            waited.steps,
            vec![
                Step::Child("Shared".to_string()),
                Step::Child("Combat".to_string()),
            ]
        );
    }

    #[test]
    fn a_chain_that_cannot_be_read_statically_is_refused_rather_than_guessed_at() {
        for expression in [
            "modules[name]",
            "Madwork.GetShared(\"Madwork\", \"MadworkMaid\")",
            "script:FindFirstAncestor(name).Thing",
            "script:WaitForChild(childName)",
        ] {
            assert!(
                parse_chain(expression).is_err(),
                "{expression} should not have parsed"
            );
        }
    }
}
