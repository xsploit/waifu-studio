//! Small shared lexical recall policy, not sentiment or semantic inference.
//! Preserve short identifiers, numbers and negation; suppress common English
//! function words. Query terms are distinct, stable and capped at 64.
use std::collections::HashSet;
const MAX_QUERY_BYTES: usize = 16 * 1024;
const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "and", "or", "is", "are", "was", "were", "be", "been", "being", "to", "of",
    "in", "on", "at", "for", "from", "with", "by", "as", "it", "its", "this", "that", "these",
    "those", "i", "me", "my", "mine", "we", "our", "ours", "you", "your", "yours", "he", "him",
    "his", "she", "her", "hers", "they", "them", "their", "theirs", "do", "does", "did", "have",
    "has", "had", "can", "could", "would", "should", "what", "which", "when", "where", "who",
    "how",
];
fn word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '\'' || c == '’'
}
fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !word_char(c))
        .filter(|word| !word.is_empty())
        .flat_map(|word| {
            let word = word
                .trim_matches(['\'', '’'])
                .to_lowercase()
                .replace('’', "'");
            let terms = if let Some(stem) = word.strip_suffix("n't") {
                // Keep explicit negation; do not introduce one-letter suffixes
                // that would match every unrelated contraction or possessive.
                let stem = match stem {
                    // Irregular auxiliary fragments are not content terms.
                    // In particular, ain't must not manufacture the acronym AI.
                    "ca" | "wo" | "sha" | "ai" => None,
                    stem => Some(stem.to_owned()),
                };
                [stem, Some("not".into())]
            } else {
                let stem = ["'s", "'re", "'ve", "'ll", "'d", "'m"]
                    .into_iter()
                    .find_map(|suffix| word.strip_suffix(suffix))
                    .unwrap_or(&word);
                [Some(stem.to_owned()), None]
            };
            terms.into_iter().flatten()
        })
        .filter(|word| !word.is_empty() && !STOP_WORDS.contains(&word.as_str()))
}
pub(crate) fn query_terms(text: &str) -> Vec<String> {
    let mut end = text.len().min(MAX_QUERY_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end < text.len() {
        // Never invent a match by treating a truncated word as a full token.
        end = text[..end].rfind(|c: char| !word_char(c)).unwrap_or(0);
    }
    let mut seen = HashSet::new();
    tokens(&text[..end])
        .filter(|term| seen.insert(term.clone()))
        .take(64)
        .collect()
}
pub(crate) fn content_terms(text: &str) -> HashSet<String> {
    tokens(text).collect()
}

/// Whole-token matching for spaced scripts. Preserve substring queries for
/// CJK syllabaries/ideographs, where whitespace is not a word boundary.
pub(crate) fn matches(content: &HashSet<String>, term: &str) -> bool {
    content.contains(term)
        || (!term.is_empty()
            && term.chars().all(|c| {
                matches!(c as u32,
        0x3400..=0x4dbf | 0x4e00..=0x9fff | 0xf900..=0xfaff |
        0x3040..=0x30ff | 0xac00..=0xd7af)
            })
            && content.iter().any(|token| token.contains(term)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn useful_terms_survive_punctuation_short_identifiers_and_negation() {
        assert_eq!(
            query_terms("What is my VR setup, VR? Not AI: 2 monitors in NY. 猫"),
            ["vr", "setup", "not", "ai", "2", "monitors", "ny", "猫"]
        );
        assert_eq!(query_terms("no never without"), ["no", "never", "without"]);
        assert!(query_terms("what are the things that you have").contains(&"things".into()));
        assert!(query_terms("the and is your").is_empty());
    }
    #[test]
    fn repeated_function_words_do_not_exhaust_the_distinct_query_budget() {
        let query = format!(
            "{} {}",
            "the VR ".repeat(100),
            (0..100)
                .map(|i| format!("term{i}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let terms = query_terms(&query);
        assert_eq!(terms.len(), 64);
        assert_eq!(terms[0], "vr");
        assert_eq!(terms[1], "term0");
        assert_eq!(terms[63], "term62");
    }
    #[test]
    fn contractions_preserve_negation_without_one_letter_suffix_noise() {
        assert_eq!(
            query_terms("Don't use honey. It isn't John's. You're here."),
            ["not", "use", "honey", "john", "here"]
        );
        assert_eq!(query_terms("won’t can’t shouldn't ain't"), ["not"]);
        assert_eq!(query_terms("O’Connor's R and C++"), ["o'connor", "r", "c"]);
    }
    #[test]
    fn query_processing_is_utf8_bounded_and_drops_partial_tokens() {
        assert!(query_terms(&format!("{} VR", "the ".repeat(10000))).is_empty());
        assert!(query_terms(&"猫".repeat(10000)).is_empty());
        let query = format!("VR {}", "猫".repeat(10000));
        assert_eq!(query_terms(&query), ["vr"]);
    }
    #[test]
    fn cjk_queries_preserve_unspaced_matching_without_latin_substring_noise() {
        let terms = content_terms("猫が好きで東京に住んでいる。 Steak for dinner");
        assert!(matches(&terms, "猫"));
        assert!(matches(&terms, "東京"));
        assert!(!matches(&terms, "tea"));
        assert!(matches(&terms, "steak"));
    }
}
