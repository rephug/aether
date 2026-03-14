#[derive(Debug, Clone, PartialEq)]
pub struct RefactorSelectionInput {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub risk_score: f64,
    pub pagerank: f64,
    pub betweenness: f64,
    pub test_count: u32,
    pub risk_factors: Vec<String>,
    pub in_cycle: bool,
    pub has_fresh_deep_sir: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefactorSymbolSelection {
    pub selected: Vec<RefactorCandidate>,
    pub forced_cycle_members: usize,
    pub skipped_fresh: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RefactorCandidate {
    pub symbol_id: String,
    pub qualified_name: String,
    pub file_path: String,
    pub refactor_risk: f64,
    pub risk_factors: Vec<String>,
    pub needs_deep_scan: bool,
    pub in_cycle: bool,
}

pub fn select_refactor_targets(
    inputs: &[RefactorSelectionInput],
    top_n: usize,
) -> RefactorSymbolSelection {
    if inputs.is_empty() || top_n == 0 {
        return RefactorSymbolSelection {
            selected: Vec::new(),
            forced_cycle_members: 0,
            skipped_fresh: 0,
        };
    }

    let max_pagerank = inputs
        .iter()
        .map(|input| input.pagerank)
        .fold(0.0_f64, f64::max);
    let max_betweenness = inputs
        .iter()
        .map(|input| input.betweenness)
        .fold(0.0_f64, f64::max);

    let mut ranked = inputs
        .iter()
        .map(|input| {
            let pagerank = normalize_signal(input.pagerank, max_pagerank);
            let betweenness = normalize_signal(input.betweenness, max_betweenness);
            let base_risk = input.risk_score.clamp(0.0, 1.0);
            let test_gap = if input.test_count == 0 {
                1.0
            } else {
                (1.0 / (input.test_count as f64 + 1.0)).clamp(0.0, 1.0)
            };
            let cycle_bonus = if input.in_cycle { 0.15 } else { 0.0 };
            let refactor_risk = (base_risk * 0.5
                + pagerank * 0.2
                + betweenness * 0.2
                + test_gap * 0.1
                + cycle_bonus)
                .clamp(0.0, 1.0);

            let mut risk_factors = input.risk_factors.clone();
            if input.in_cycle && !risk_factors.iter().any(|factor| factor == "cycle_member") {
                risk_factors.push("cycle_member".to_owned());
            }
            if input.test_count == 0
                && !risk_factors
                    .iter()
                    .any(|factor| factor == "missing_test_coverage")
            {
                risk_factors.push("missing_test_coverage".to_owned());
            }

            RefactorCandidate {
                symbol_id: input.symbol_id.clone(),
                qualified_name: input.qualified_name.clone(),
                file_path: input.file_path.clone(),
                refactor_risk,
                risk_factors,
                needs_deep_scan: !input.has_fresh_deep_sir,
                in_cycle: input.in_cycle,
            }
        })
        .collect::<Vec<_>>();

    ranked.sort_by(|left, right| {
        right
            .refactor_risk
            .total_cmp(&left.refactor_risk)
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.qualified_name.cmp(&right.qualified_name))
            .then_with(|| left.symbol_id.cmp(&right.symbol_id))
    });

    let mut selected = Vec::new();
    for candidate in ranked.iter().filter(|candidate| candidate.in_cycle) {
        selected.push(candidate.clone());
    }
    for candidate in ranked {
        if selected
            .iter()
            .any(|existing| existing.symbol_id == candidate.symbol_id)
        {
            continue;
        }
        if selected.len() >= top_n {
            break;
        }
        selected.push(candidate);
    }

    let forced_cycle_members = selected
        .iter()
        .filter(|candidate| candidate.in_cycle)
        .count();
    let skipped_fresh = selected
        .iter()
        .filter(|candidate| !candidate.needs_deep_scan)
        .count();

    RefactorSymbolSelection {
        selected,
        forced_cycle_members,
        skipped_fresh,
    }
}

fn normalize_signal(value: f64, max_value: f64) -> f64 {
    if max_value <= f64::EPSILON {
        0.0
    } else {
        (value / max_value).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::{RefactorSelectionInput, RefactorSymbolSelection, select_refactor_targets};

    fn input(symbol_id: &str) -> RefactorSelectionInput {
        RefactorSelectionInput {
            symbol_id: symbol_id.to_owned(),
            qualified_name: format!("demo::{symbol_id}"),
            file_path: "crates/demo/src/lib.rs".to_owned(),
            risk_score: 0.2,
            pagerank: 0.1,
            betweenness: 0.1,
            test_count: 1,
            risk_factors: vec!["baseline".to_owned()],
            in_cycle: false,
            has_fresh_deep_sir: false,
        }
    }

    fn selected_ids(selection: &RefactorSymbolSelection) -> Vec<&str> {
        selection
            .selected
            .iter()
            .map(|candidate| candidate.symbol_id.as_str())
            .collect()
    }

    #[test]
    fn empty_health_inputs_return_empty_selection() {
        let selection = select_refactor_targets(&[], 20);
        assert!(selection.selected.is_empty());
        assert_eq!(selection.forced_cycle_members, 0);
        assert_eq!(selection.skipped_fresh, 0);
    }

    #[test]
    fn cycle_members_are_force_included_even_below_top_n() {
        let mut top = input("sym-top");
        top.risk_score = 0.95;
        top.pagerank = 0.95;

        let mut cycle_a = input("sym-cycle-a");
        cycle_a.risk_score = 0.1;
        cycle_a.in_cycle = true;

        let mut cycle_b = input("sym-cycle-b");
        cycle_b.risk_score = 0.08;
        cycle_b.in_cycle = true;

        let selection = select_refactor_targets(&[top, cycle_a, cycle_b], 1);
        assert_eq!(selection.forced_cycle_members, 2);
        assert_eq!(selected_ids(&selection), vec!["sym-cycle-a", "sym-cycle-b"]);
    }

    #[test]
    fn fresh_deep_sir_symbols_do_not_need_deep_scan() {
        let mut fresh = input("sym-fresh");
        fresh.has_fresh_deep_sir = true;
        fresh.risk_score = 0.9;

        let selection = select_refactor_targets(&[fresh], 5);
        assert_eq!(selection.selected.len(), 1);
        assert!(!selection.selected[0].needs_deep_scan);
        assert_eq!(selection.skipped_fresh, 1);
    }

    #[test]
    fn top_n_respects_limit_when_cycles_do_not_expand_selection() {
        let mut a = input("sym-a");
        a.risk_score = 0.9;
        let mut b = input("sym-b");
        b.risk_score = 0.8;
        let mut c = input("sym-c");
        c.risk_score = 0.7;

        let selection = select_refactor_targets(&[a, b, c], 2);
        assert_eq!(selection.selected.len(), 2);
        assert_eq!(selected_ids(&selection), vec!["sym-a", "sym-b"]);
    }
}
