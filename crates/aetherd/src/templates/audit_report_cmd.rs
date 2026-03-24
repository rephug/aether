use super::TemplateContext;

#[derive(Debug, Clone, Copy, Default)]
pub struct AuditReportCommandTemplate;

impl AuditReportCommandTemplate {
    pub fn render(_context: &TemplateContext) -> String {
        r#"---
description: Show all audit findings from previous sessions
---

Retrieve and display all audit findings.

## If `aether_audit_report` is available:
Call `aether_audit_report` to retrieve all open findings.
If the user specifies a crate, severity, or status filter, pass those parameters.

## Otherwise:
Call `aether_recall` with query "AUDIT FINDING" to find stored findings.
Parse the structured notes and group by crate, then by severity.

## Display format
Show findings grouped by crate, then severity (critical -> high -> medium -> low).
For each finding show: symbol, file, severity, category, description, certainty, status.
End with a summary table of counts by severity.
"#
        .to_owned()
    }
}
