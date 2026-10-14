---
argument-hint: [--fix] [--enrich] [--audit]
description: Show what to work on next based on AETHER health scores
---

Analyze the codebase and recommend the next action.

## Step 1: Get the health landscape

Call `aether_health_hotspots` with limit 10.

Present a simple ranked table:

```
Crate Health Priority
──────────────────────────────
aether-store     48/100  ← WORK HERE
aetherd          55/100
aether-mcp       62/100
aether-analysis  71/100
```

## Step 2: Check existing findings

Call `aether_audit_report` with status "open".

Count open findings by crate. Report:

```
Open findings:
  aether-store:  12 (3 high, 5 medium, 4 low)
  aetherd:        7 (2 high, 3 medium, 2 low)
  aether-mcp:     0
```

## Step 3: Check enrichment coverage

Run this query via bash:
```bash
sqlite3 .aether/meta.sqlite "
SELECT
  SUBSTR(s.file_path, 1, INSTR(s.file_path, '/src/') - 1) as crate,
  COUNT(*) as total,
  SUM(CASE WHEN sir.generation_pass = 'deep' THEN 1 ELSE 0 END) as enriched,
  ROUND(100.0 * SUM(CASE WHEN sir.generation_pass = 'deep' THEN 1 ELSE 0 END) / COUNT(*), 0) as pct
FROM sir
JOIN symbols s ON s.id = sir.id
GROUP BY crate
ORDER BY pct ASC
LIMIT 5;
"
```

## Step 4: Recommend next action

Based on the data, recommend ONE action:

**If the worst crate has no audit findings:**
→ "Run `/audit <worst-crate>` to find bugs first."

**If the worst crate has open findings but no enrichment:**
→ "Run `/enrich <worst-crate> --top 30` to deepen analysis."

**If the worst crate has findings AND enrichment:**
→ "Run `/refactor <worst-crate>` to get a fix plan, then fix the top finding."

**If the worst crate has findings that are all fixed:**
→ "Run `/audit-changes HEAD~5` to verify fixes, then move to the next crate."

**If $ARGUMENTS contains --fix:**
→ After the recommendation, automatically start fixing the highest-severity
  open finding in the worst crate.

**If $ARGUMENTS contains --enrich:**
→ After the recommendation, automatically run enrichment on the worst crate.

**If $ARGUMENTS contains --audit:**
→ After the recommendation, automatically run audit on the worst crate.

## Presentation

Keep it simple. The user should understand what to do in 10 seconds:

```
╔══════════════════════════════════════════════╗
║  NEXT: Audit aetherd (55/100, 0 findings)   ║
║                                              ║
║  Run: /audit aetherd                         ║
║                                              ║
║  Why: Lowest health score without findings.  ║
║  aether-store already has 12 open findings   ║
║  — fix those first, or audit the next crate. ║
╚══════════════════════════════════════════════╝
```
