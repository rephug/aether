//! Advisory evidence checks, separate from JSON/schema validation.
//!
//! These checks establish that claim references are well formed and in the supplied
//! excerpt. They cannot establish that a referenced line supports a claim's meaning.
//! Never reject or rewrite a SIR, adjust confidence, or trigger another model call here.

use aether_sir::SirAnnotation;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SirEvidenceContext {
    pub supplied_lines: usize,
    pub source_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceIssue {
    MissingSourceReference { field: &'static str, index: usize },
    InvalidSourceReference { field: &'static str, index: usize },
}

/// Audit only fields that make behavioral effect/failure claims. `Unknown:` is an
/// explicit uncertainty convention, not a blacklist of natural-language words.
pub fn audit_sir_evidence(sir: &SirAnnotation, context: SirEvidenceContext) -> Vec<EvidenceIssue> {
    let mut issues = Vec::new();
    for (field, claims) in [
        ("side_effects", &sir.side_effects),
        ("error_modes", &sir.error_modes),
    ] {
        for (index, claim) in claims.iter().enumerate() {
            let unknown = claim
                .trim_start()
                .get(..8)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("Unknown:"));
            let mut references = claim.match_indices("[source:").peekable();
            if references.peek().is_none() {
                if !unknown {
                    issues.push(EvidenceIssue::MissingSourceReference { field, index });
                }
                continue;
            }
            if references
                .any(|(offset, _)| !valid_reference(&claim[offset..], context.supplied_lines))
            {
                issues.push(EvidenceIssue::InvalidSourceReference { field, index });
            }
        }
    }
    issues
}

fn valid_reference(reference: &str, supplied_lines: usize) -> bool {
    let Some(contents) = reference
        .strip_prefix("[source:")
        .and_then(|s| s.split_once(']').map(|(v, _)| v))
    else {
        return false;
    };
    let (start, end) = contents.split_once('-').unwrap_or((contents, contents));
    let parse = |s: &str| s.strip_prefix('L').and_then(|s| s.parse::<usize>().ok());
    matches!((parse(start), parse(end)), (Some(start), Some(end)) if start > 0 && start <= end && end <= supplied_lines)
}

/// Log issues without claim text/source contents, while preserving legacy records.
pub fn log_sir_evidence_issues(sir: &SirAnnotation, context: SirEvidenceContext, symbol: &str) {
    let issues = audit_sir_evidence(sir, context);
    if !issues.is_empty() {
        tracing::warn!(symbol, issues = ?issues, source_truncated = context.source_truncated,
            "SIR evidence-reference audit needs review; JSON validity and valid references do not establish factual accuracy");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sir(claim: &str) -> SirAnnotation {
        SirAnnotation {
            intent: "Fixture".into(),
            behavior: None,
            inputs: vec![],
            outputs: vec![],
            side_effects: vec![claim.into()],
            dependencies: vec![],
            error_modes: vec![],
            confidence: 0.9,
            edge_cases: None,
            complexity: None,
            method_dependencies: None,
        }
    }

    #[test]
    fn evidence_policy_confusion_matrix_on_claim_references() {
        let context = SirEvidenceContext {
            supplied_lines: 4,
            source_truncated: false,
        };
        // Positive/negative are about citation defects, never factual correctness.
        let cases = [
            ("Writes a denylist [source:L2]", false),
            ("Returns an allowlist [source:L1-L3]", false),
            ("Unknown: callee implementation is unavailable", false),
            ("unknown: missing tail", false),
            ("May write a file [source:L4]", false),
            ("Does not perform network I/O [source:L1-L4]", false),
            ("Writes a file", true),
            ("Writes a file [source:L0]", true),
            ("Writes a file [source:L5]", true),
            ("Writes a file [source:L3-L2]", true),
            ("Writes a file [source:L2", true),
            ("Writes a file [source:L2] [source:L99]", true),
            ("Unknown behavior writes a file", true),
            ("Writes a file [source:Lx]", true),
            ("Unknown: missing tail [source:L99]", true),
        ];
        let (mut tp, mut tn, mut fp, mut fn_) = (0, 0, 0, 0);
        for (claim, defect) in cases {
            let detected = !audit_sir_evidence(&sir(claim), context).is_empty();
            match (defect, detected) {
                (true, true) => tp += 1,
                (false, false) => tn += 1,
                (false, true) => fp += 1,
                (true, false) => fn_ += 1,
            }
        }
        assert_eq!((tp, tn, fp, fn_), (9, 6, 0, 0));
    }

    #[test]
    fn valid_citation_is_not_semantic_proof_and_audit_does_not_mutate() {
        let annotation = sir("Invented claim with an in-range citation [source:L2]");
        let original = annotation.clone();
        assert!(
            audit_sir_evidence(
                &annotation,
                SirEvidenceContext {
                    supplied_lines: 3,
                    source_truncated: true
                }
            )
            .is_empty()
        );
        assert_eq!(annotation, original);
    }

    #[test]
    fn legacy_sir_remains_schema_valid_without_evidence_references() {
        let annotation = sir("Old summary without a reference");
        assert!(aether_sir::validate_sir(&annotation).is_ok());
        assert_eq!(
            audit_sir_evidence(
                &annotation,
                SirEvidenceContext {
                    supplied_lines: 1,
                    source_truncated: false
                }
            )
            .len(),
            1
        );
    }
}
