---
description: A 30-second overview of index coverage, SIR confidence and the least healthy crates
---

Give a short session-start health check of this codebase. It must take less than 30
seconds to read. This command doesn't edit files or inject SIRs.

## Step 1: Gather
1. Call `aether_status`. If `symbol_count` is 0 (or `store_present` is false), the
   workspace has not been indexed: say so, suggest `aetherd --workspace . --index-once`
   (plus the Zero-Key Onboarding steps in CLAUDE.md for SIRs), and stop. Otherwise note
   `symbol_count` and `sir_coverage`. Its `percentage` is a fraction: 0.82 means 82%.
2. Summarize SIR confidence with this read-only query:

   ```bash
   sqlite3 -readonly .aether/meta.sqlite "
   SELECT
     COUNT(*) AS sirs,
     ROUND(AVG(json_extract(sir.sir_json, '$.confidence')), 2) AS avg_confidence,
     SUM(json_extract(sir.sir_json, '$.intent') LIKE '[MOCK]%') AS placeholders,
     SUM(json_extract(sir.sir_json, '$.confidence') < 0.5
         AND json_extract(sir.sir_json, '$.intent') NOT LIKE '[MOCK]%') AS other_below_half,
     SUM(sir.generation_pass = 'deep') AS deep
   FROM sir
   JOIN symbols s ON s.id = sir.id
   WHERE COALESCE(TRIM(sir.sir_json), '') <> '';
   "
   ```

   Without the `sqlite3` CLI, run the same query with Python's `sqlite3` module
   (`sqlite3.connect('file:.aether/meta.sqlite?mode=ro', uri=True)`). If neither works,
   leave the confidence line out.
3. Call `aether_health_hotspots` with `limit` set to 5. It lists only crates scoring 75
   or below (out of 100, where higher is healthier; 50 and below is serious), each with
   its top violation.
4. Call `aether_health` with `include` set to `["cycles", "risk_hotspots"]` and `limit`
   set to 5. Note `analysis.cycles_detected` and the first entry of `risk_hotspots`
   (the riskiest symbol).

## Step 2: Report
If coverage is at least 90%, there are no placeholders, the average confidence is at
least 0.7, and `aether_health_hotspots` lists no crate, print a single line such as
"All healthy: 1,500 symbols, 96% with SIRs, average confidence 0.86, no crate below 76."
and stop.

Otherwise print exactly this shape, leaving out lines you have no data for:

```
Health check
Coverage:   1,234 of 1,500 symbols have a SIR (82%)
Confidence: average 0.71 · 40 placeholders · 80 others below 0.5 · 300 deep
Crates needing attention (score out of 100, lower is worse):
  <crate>  48  Moderate  <top violation, a few words>
Structure:  2 dependency cycles · riskiest symbol: <name> (<file>)
Next:
  1. <recommendation>
```

- List at most 5 crates (those `aether_health_hotspots` returned), and give
  at most 3 recommendations.
- Each recommendation is one line naming a command and its reason: `/scan <crate>` for
  placeholders or missing SIRs, `/enrich <crate-name>` for low confidence, `/audit
  <crate>` or `/refactor <crate>` for a low health score, `/next` for a fuller plan.
- Do not show raw tool output, symbol ids or lists of files.
