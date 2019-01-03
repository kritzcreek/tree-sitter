use crate::build_tables::item::LookaheadSet;
use crate::grammars::LexicalGrammar;
use crate::nfa::{CharacterSet, NfaCursor};
use hashbrown::HashSet;
use std::cmp::Ordering;
use std::fmt;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TokenConflictStatus {
    does_overlap: bool,
    does_match_valid_continuation: bool,
    does_match_separators: bool,
    matches_same_string: bool,
}

pub(crate) struct TokenConflictMap<'a> {
    n: usize,
    status_matrix: Vec<TokenConflictStatus>,
    starting_chars_by_index: Vec<CharacterSet>,
    following_chars_by_index: Vec<CharacterSet>,
    grammar: &'a LexicalGrammar,
}

impl<'a> TokenConflictMap<'a> {
    pub fn new(grammar: &'a LexicalGrammar, following_tokens: Vec<LookaheadSet>) -> Self {
        let mut cursor = NfaCursor::new(&grammar.nfa, Vec::new());
        let starting_chars = get_starting_chars(&mut cursor, grammar);
        let following_chars = get_following_chars(&starting_chars, following_tokens);

        let n = grammar.variables.len();
        let mut status_matrix = vec![TokenConflictStatus::default(); n * n];
        for i in 0..grammar.variables.len() {
            for j in 0..i {
                let status = compute_conflict_status(&mut cursor, grammar, &following_chars, i, j);
                status_matrix[matrix_index(n, i, j)] = status.0;
                status_matrix[matrix_index(n, j, i)] = status.1;
            }
        }

        TokenConflictMap {
            n,
            status_matrix,
            starting_chars_by_index: starting_chars,
            following_chars_by_index: following_chars,
            grammar,
        }
    }

    pub fn has_same_conflict_status(&self, a: usize, b: usize, other: usize) -> bool {
        let left = &self.status_matrix[matrix_index(self.n, a, other)];
        let right = &self.status_matrix[matrix_index(self.n, b, other)];
        left == right
    }

    pub fn does_match_same_string(&self, i: usize, j: usize) -> bool {
        self.status_matrix[matrix_index(self.n, i, j)].matches_same_string
    }

    pub fn does_conflict(&self, i: usize, j: usize) -> bool {
        let entry = &self.status_matrix[matrix_index(self.n, i, j)];
        entry.does_match_valid_continuation || entry.does_match_separators
    }

    pub fn does_overlap(&self, i: usize, j: usize) -> bool {
        self.status_matrix[matrix_index(self.n, i, j)].does_overlap
    }

    pub fn prefer_token(grammar: &LexicalGrammar, left: (i32, usize), right: (i32, usize)) -> bool {
        if left.0 > right.0 {
            return true;
        } else if left.0 < right.0 {
            return false;
        }

        match grammar.variables[left.1]
            .implicit_precedence
            .cmp(&grammar.variables[right.1].implicit_precedence)
        {
            Ordering::Less => false,
            Ordering::Greater => true,
            Ordering::Equal => left.1 < right.1,
        }
    }
}

impl<'a> fmt::Debug for TokenConflictMap<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "TokenConflictMap {{\n")?;

        write!(f, "  starting_characters: {{\n")?;
        for i in 0..self.n {
            write!(f, "    {}: {:?},\n", i, self.starting_chars_by_index[i])?;
        }
        write!(f, "  }},\n")?;

        write!(f, "  following_characters: {{\n")?;
        for i in 0..self.n {
            write!(
                f,
                "    {}: {:?},\n",
                self.grammar.variables[i].name, self.following_chars_by_index[i]
            )?;
        }
        write!(f, "  }},\n")?;

        write!(f, "  status_matrix: {{\n")?;
        for i in 0..self.n {
            write!(f, "    {}: {{\n", self.grammar.variables[i].name)?;
            for j in 0..self.n {
                write!(
                    f,
                    "      {}: {:?},\n",
                    self.grammar.variables[j].name,
                    self.status_matrix[matrix_index(self.n, i, j)]
                )?;
            }
            write!(f, "    }},\n")?;
        }
        write!(f, "  }},")?;
        write!(f, "}}")?;
        Ok(())
    }
}

fn matrix_index(variable_count: usize, i: usize, j: usize) -> usize {
    variable_count * i + j
}

fn get_starting_chars(cursor: &mut NfaCursor, grammar: &LexicalGrammar) -> Vec<CharacterSet> {
    let mut result = Vec::with_capacity(grammar.variables.len());
    for variable in &grammar.variables {
        cursor.reset(vec![variable.start_state]);
        let mut all_chars = CharacterSet::empty();
        for (chars, _, _, _) in cursor.successors() {
            all_chars = all_chars.add(chars);
        }
        result.push(all_chars);
    }
    result
}

fn get_following_chars(
    starting_chars: &Vec<CharacterSet>,
    following_tokens: Vec<LookaheadSet>,
) -> Vec<CharacterSet> {
    following_tokens
        .into_iter()
        .map(|following_tokens| {
            let mut chars = CharacterSet::empty();
            for token in following_tokens.iter() {
                if token.is_terminal() {
                    chars = chars.add(&starting_chars[token.index]);
                }
            }
            chars
        })
        .collect()
}

fn compute_conflict_status(
    cursor: &mut NfaCursor,
    grammar: &LexicalGrammar,
    following_chars: &Vec<CharacterSet>,
    i: usize,
    j: usize,
) -> (TokenConflictStatus, TokenConflictStatus) {
    let mut visited_state_sets = HashSet::new();
    let mut state_set_queue = vec![vec![
        grammar.variables[i].start_state,
        grammar.variables[j].start_state,
    ]];
    let mut result = (
        TokenConflictStatus::default(),
        TokenConflictStatus::default(),
    );

    while let Some(state_set) = state_set_queue.pop() {
        // Don't pursue states where there's no potential for conflict.
        if variable_ids_for_states(&state_set, grammar).count() > 1 {
            cursor.reset(state_set);
        } else {
            continue;
        }

        let mut completion = None;
        for (id, precedence) in cursor.completions() {
            if let Some((prev_id, prev_precedence)) = completion {
                if id == prev_id {
                    continue;
                }

                // Prefer tokens with higher precedence. For tokens with equal precedence,
                // prefer those listed earlier in the grammar.
                let winning_id;
                if TokenConflictMap::prefer_token(
                    grammar,
                    (prev_precedence, prev_id),
                    (precedence, id),
                ) {
                    winning_id = prev_id;
                } else {
                    winning_id = id;
                    completion = Some((id, precedence));
                }

                if winning_id == i {
                    result.0.matches_same_string = true;
                    result.0.does_overlap = true;
                } else {
                    result.1.matches_same_string = true;
                    result.1.does_overlap = true;
                }
            } else {
                completion = Some((id, precedence));
            }
        }

        for (chars, advance_precedence, next_states, in_sep) in cursor.grouped_successors() {
            let mut can_advance = true;
            if let Some((completed_id, completed_precedence)) = completion {
                let mut other_id = None;
                let mut successor_contains_completed_id = false;
                for variable_id in variable_ids_for_states(&next_states, grammar) {
                    if variable_id == completed_id {
                        successor_contains_completed_id = true;
                        break;
                    } else {
                        other_id = Some(variable_id);
                    }
                }

                if let (Some(other_id), false) = (other_id, successor_contains_completed_id) {
                    let winning_id;
                    if advance_precedence < completed_precedence {
                        winning_id = completed_id;
                        can_advance = false;
                    } else {
                        winning_id = other_id;
                    }

                    if winning_id == i {
                        result.0.does_overlap = true;
                        if chars.does_intersect(&following_chars[j]) {
                            result.0.does_match_valid_continuation = true;
                        }
                        if in_sep {
                            result.0.does_match_separators = true;
                        }
                    } else {
                        result.1.does_overlap = true;
                        if chars.does_intersect(&following_chars[i]) {
                            result.1.does_match_valid_continuation = true;
                        }
                    }
                }
            }

            if can_advance && visited_state_sets.insert(next_states.clone()) {
                state_set_queue.push(next_states);
            }
        }
    }
    result
}

fn variable_ids_for_states<'a>(
    state_ids: &'a Vec<u32>,
    grammar: &'a LexicalGrammar,
) -> impl Iterator<Item = usize> + 'a {
    let mut prev = None;
    state_ids.iter().filter_map(move |state_id| {
        let variable_id = grammar.variable_index_for_nfa_state(*state_id);
        if prev != Some(variable_id) {
            prev = Some(variable_id);
            prev
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grammars::{Variable, VariableType};
    use crate::prepare_grammar::{expand_tokens, ExtractedLexicalGrammar};
    use crate::rules::{Rule, Symbol};

    #[test]
    fn test_starting_characters() {
        let grammar = expand_tokens(ExtractedLexicalGrammar {
            separators: Vec::new(),
            variables: vec![
                Variable {
                    name: "token_0".to_string(),
                    kind: VariableType::Named,
                    rule: Rule::pattern("[a-f]1|0x\\d"),
                },
                Variable {
                    name: "token_1".to_string(),
                    kind: VariableType::Named,
                    rule: Rule::pattern("d*ef"),
                },
            ],
        })
        .unwrap();

        let token_map = TokenConflictMap::new(&grammar, Vec::new());

        assert_eq!(
            token_map.starting_chars_by_index[0],
            CharacterSet::empty().add_range('a', 'f').add_char('0')
        );
        assert_eq!(
            token_map.starting_chars_by_index[1],
            CharacterSet::empty().add_range('d', 'e')
        );
    }

    #[test]
    fn test_token_conflicts() {
        let grammar = expand_tokens(ExtractedLexicalGrammar {
            separators: Vec::new(),
            variables: vec![
                Variable {
                    name: "in".to_string(),
                    kind: VariableType::Named,
                    rule: Rule::string("in"),
                },
                Variable {
                    name: "identifier".to_string(),
                    kind: VariableType::Named,
                    rule: Rule::pattern("\\w+"),
                },
                Variable {
                    name: "instanceof".to_string(),
                    kind: VariableType::Named,
                    rule: Rule::string("instanceof"),
                },
            ],
        })
        .unwrap();

        let var = |name| index_of_var(&grammar, name);

        let token_map = TokenConflictMap::new(
            &grammar,
            vec![
                LookaheadSet::with([Symbol::terminal(var("identifier"))].iter().cloned()),
                LookaheadSet::with([Symbol::terminal(var("in"))].iter().cloned()),
                LookaheadSet::with([Symbol::terminal(var("identifier"))].iter().cloned()),
            ],
        );

        // Given the string "in", the `in` token is preferred over the `identifier` token
        assert!(token_map.does_match_same_string(var("in"), var("identifier")));
        assert!(!token_map.does_match_same_string(var("identifier"), var("in")));

        // Depending on what character follows, the string "in" may be treated as part of an
        // `identifier` token.
        assert!(token_map.does_conflict(var("identifier"), var("in")));

        // Depending on what character follows, the string "instanceof" may be treated as part of
        // an `identifier` token.
        assert!(token_map.does_conflict(var("identifier"), var("instanceof")));
        assert!(token_map.does_conflict(var("instanceof"), var("in")));
    }

    fn index_of_var(grammar: &LexicalGrammar, name: &str) -> usize {
        grammar
            .variables
            .iter()
            .position(|v| v.name == name)
            .unwrap()
    }
}
