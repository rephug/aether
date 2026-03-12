pub(crate) fn project_note_lexical_terms(query: &str) -> Vec<String> {
    let mut terms = Vec::new();

    for token in query
        .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '/'))
        .map(str::trim)
        .filter(|token| !token.is_empty())
    {
        let token = token.to_ascii_lowercase();
        if !terms.iter().any(|existing| existing == &token) {
            terms.push(token);
        }
    }

    if terms.is_empty() && !query.trim().is_empty() {
        terms.push(query.trim().to_ascii_lowercase());
    }

    terms
}
