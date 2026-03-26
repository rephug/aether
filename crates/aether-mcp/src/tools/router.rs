use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ErrorData as McpError, Json, ServerHandler, tool, tool_handler, tool_router};

use super::{
    AetherAcknowledgeDriftRequest, AetherAcknowledgeDriftResponse, AetherAskRequest,
    AetherAskResponse, AetherAuditCandidatesRequest, AetherAuditCandidatesResponse,
    AetherAuditCrossSymbolRequest, AetherAuditCrossSymbolResponse, AetherAuditReportRequest,
    AetherAuditReportResponse, AetherAuditResolveRequest, AetherAuditResolveResponse,
    AetherAuditSubmitRequest, AetherAuditSubmitResponse, AetherBlastRadiusRequest,
    AetherBlastRadiusResponse, AetherCallChainRequest, AetherCallChainResponse,
    AetherContractAddRequest, AetherContractAddResponse, AetherContractCheckRequest,
    AetherContractCheckResponse, AetherContractDismissRequest, AetherContractDismissResponse,
    AetherContractListRequest, AetherContractListResponse, AetherContractRemoveRequest,
    AetherContractRemoveResponse, AetherContractViolationsRequest,
    AetherContractViolationsResponse, AetherDependenciesRequest, AetherDependenciesResponse,
    AetherDriftReportRequest, AetherDriftReportResponse, AetherEnhancePromptRequest,
    AetherEnhancePromptResponse, AetherExplainRequest, AetherExplainResponse, AetherGetSirRequest,
    AetherGetSirResponse, AetherHealthExplainRequest, AetherHealthHotspotsRequest,
    AetherHealthRequest, AetherHealthResponse, AetherMcpServer, AetherRecallRequest,
    AetherRecallResponse, AetherRefactorPrepRequest, AetherRefactorPrepResponse,
    AetherRememberRequest, AetherRememberResponse, AetherSearchRequest, AetherSearchResponse,
    AetherSessionNoteResponse, AetherSirContextRequest, AetherSirContextResponse,
    AetherSirInjectRequest, AetherSirInjectResponse, AetherStatusResponse,
    AetherSuggestTraitSplitRequest, AetherSuggestTraitSplitResponse, AetherSymbolLookupRequest,
    AetherSymbolLookupResponse, AetherSymbolTimelineRequest, AetherSymbolTimelineResponse,
    AetherTestIntentsRequest, AetherTestIntentsResponse, AetherTextResponse,
    AetherTraceCauseRequest, AetherTraceCauseResponse, AetherUsageMatrixRequest,
    AetherUsageMatrixResponse, AetherVerifyIntentRequest, AetherVerifyIntentResponse,
    AetherWhyChangedRequest, AetherWhyChangedResponse, SERVER_DESCRIPTION, SERVER_NAME,
    SERVER_VERSION,
};
#[cfg(feature = "verification")]
use super::{AetherVerifyRequest, AetherVerifyResponse};
use crate::AetherMcpError;

fn to_mcp_error(err: AetherMcpError) -> McpError {
    McpError::internal_error(err.to_string(), None)
}

#[tool_router(router = tool_router, vis = "pub(crate)")]
impl AetherMcpServer {
    #[tool(name = "aether_status", description = "Get AETHER local store status")]
    pub async fn aether_status(&self) -> Result<Json<AetherStatusResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_status");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_status_logic())
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_symbol_lookup",
        description = "Lookup symbols by qualified name or file path"
    )]
    pub async fn aether_symbol_lookup(
        &self,
        Parameters(request): Parameters<AetherSymbolLookupRequest>,
    ) -> Result<Json<AetherSymbolLookupResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_symbol_lookup");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_symbol_lookup_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_dependencies",
        description = "Get resolved callers and call dependencies for a symbol"
    )]
    pub async fn aether_dependencies(
        &self,
        Parameters(request): Parameters<AetherDependenciesRequest>,
    ) -> Result<Json<AetherDependenciesResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_dependencies");
        self.aether_dependencies_logic(request)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_enhance_prompt",
        description = "Enhance a coding prompt with AETHER codebase intelligence context"
    )]
    pub async fn aether_enhance_prompt(
        &self,
        Parameters(request): Parameters<AetherEnhancePromptRequest>,
    ) -> Result<Json<AetherEnhancePromptResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_enhance_prompt");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_enhance_prompt_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_usage_matrix",
        description = "Get a consumer-by-method usage matrix for a trait or struct, showing which files call which methods and suggesting method clusters for trait decomposition"
    )]
    pub async fn aether_usage_matrix(
        &self,
        Parameters(request): Parameters<AetherUsageMatrixRequest>,
    ) -> Result<Json<AetherUsageMatrixResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_usage_matrix");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_usage_matrix_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_suggest_trait_split",
        description = "Suggest how to decompose a large trait or struct into smaller capability groups based on consumer usage patterns"
    )]
    pub async fn aether_suggest_trait_split(
        &self,
        Parameters(request): Parameters<AetherSuggestTraitSplitRequest>,
    ) -> Result<Json<AetherSuggestTraitSplitResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_suggest_trait_split");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_suggest_trait_split_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_call_chain",
        description = "Get transitive call-chain levels for a symbol"
    )]
    pub async fn aether_call_chain(
        &self,
        Parameters(request): Parameters<AetherCallChainRequest>,
    ) -> Result<Json<AetherCallChainResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_call_chain");
        self.aether_call_chain_logic(request)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_search",
        description = "Search symbols by name, path, language, or kind"
    )]
    pub async fn aether_search(
        &self,
        Parameters(request): Parameters<AetherSearchRequest>,
    ) -> Result<Json<AetherSearchResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_search");
        self.aether_search_logic(request)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_remember",
        description = "Store project memory note content with deterministic deduplication"
    )]
    pub async fn aether_remember(
        &self,
        Parameters(request): Parameters<AetherRememberRequest>,
    ) -> Result<Json<AetherRememberResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_remember");
        self.aether_remember_logic(request)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_session_note",
        description = "Capture an in-session project note with source_type=session"
    )]
    pub async fn aether_session_note(
        &self,
        Parameters(request): Parameters<AetherRememberRequest>,
    ) -> Result<Json<AetherSessionNoteResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_session_note");
        self.aether_session_note_logic(request)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_recall",
        description = "Recall project memory notes using lexical, semantic, or hybrid retrieval"
    )]
    pub async fn aether_recall(
        &self,
        Parameters(request): Parameters<AetherRecallRequest>,
    ) -> Result<Json<AetherRecallResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_recall");
        self.aether_recall_logic(request)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_ask",
        description = "Search symbols, notes, coupling, and test intents with unified ranking"
    )]
    pub async fn aether_ask(
        &self,
        Parameters(request): Parameters<AetherAskRequest>,
    ) -> Result<Json<AetherAskResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_ask");
        self.aether_ask_logic(request)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_audit_candidates",
        description = "Get ranked list of symbols most in need of deep audit review, combining structural risk with SIR confidence and reasoning trace uncertainty"
    )]
    pub async fn aether_audit_candidates(
        &self,
        Parameters(request): Parameters<AetherAuditCandidatesRequest>,
    ) -> Result<Json<AetherAuditCandidatesResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_audit_candidates");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_audit_candidates_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_audit_cross_symbol",
        description = "Trace callers and callees from a root symbol with full SIR, source, and reasoning context for cross-boundary audit analysis"
    )]
    pub async fn aether_audit_cross_symbol(
        &self,
        Parameters(request): Parameters<AetherAuditCrossSymbolRequest>,
    ) -> Result<Json<AetherAuditCrossSymbolResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_audit_cross_symbol");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_audit_cross_symbol_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_audit_submit",
        description = "Submit a structured audit finding for a symbol"
    )]
    pub async fn aether_audit_submit(
        &self,
        Parameters(request): Parameters<AetherAuditSubmitRequest>,
    ) -> Result<Json<AetherAuditSubmitResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_audit_submit");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_audit_submit_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_audit_report",
        description = "Query audit findings by crate, severity, category, or status"
    )]
    pub async fn aether_audit_report(
        &self,
        Parameters(request): Parameters<AetherAuditReportRequest>,
    ) -> Result<Json<AetherAuditReportResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_audit_report");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_audit_report_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_audit_resolve",
        description = "Mark an audit finding as fixed, wontfix, or confirmed"
    )]
    pub async fn aether_audit_resolve(
        &self,
        Parameters(request): Parameters<AetherAuditResolveRequest>,
    ) -> Result<Json<AetherAuditResolveResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_audit_resolve");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_audit_resolve_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_contract_add",
        description = "Add a behavioral contract (must/must_not/preserves) on a symbol"
    )]
    pub async fn aether_contract_add(
        &self,
        Parameters(request): Parameters<AetherContractAddRequest>,
    ) -> Result<Json<AetherContractAddResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_contract_add");
        self.state.require_writable().map_err(to_mcp_error)?;
        let embedding_json = self
            .maybe_embed_contract_clause(request.clause_text.as_str())
            .await;
        let server = self.clone();
        tokio::task::spawn_blocking(move || {
            server.aether_contract_add_logic(request, embedding_json)
        })
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_contract_list",
        description = "List active intent contracts, optionally filtered by symbol"
    )]
    pub async fn aether_contract_list(
        &self,
        Parameters(request): Parameters<AetherContractListRequest>,
    ) -> Result<Json<AetherContractListResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_contract_list");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_contract_list_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_contract_remove",
        description = "Deactivate an intent contract by ID"
    )]
    pub async fn aether_contract_remove(
        &self,
        Parameters(request): Parameters<AetherContractRemoveRequest>,
    ) -> Result<Json<AetherContractRemoveResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_contract_remove");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_contract_remove_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_contract_check",
        description = "Verify intent contracts against current SIR using embedding similarity"
    )]
    pub async fn aether_contract_check(
        &self,
        Parameters(request): Parameters<AetherContractCheckRequest>,
    ) -> Result<Json<AetherContractCheckResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_contract_check");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_contract_check_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_contract_violations",
        description = "Query contract violation history by symbol or contract"
    )]
    pub async fn aether_contract_violations(
        &self,
        Parameters(request): Parameters<AetherContractViolationsRequest>,
    ) -> Result<Json<AetherContractViolationsResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_contract_violations");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_contract_violations_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_contract_dismiss",
        description = "Dismiss a contract violation with a reason"
    )]
    pub async fn aether_contract_dismiss(
        &self,
        Parameters(request): Parameters<AetherContractDismissRequest>,
    ) -> Result<Json<AetherContractDismissResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_contract_dismiss");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_contract_dismiss_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_sir_inject",
        description = "Inject or update a symbol's complete SIR annotation. Accepts intent, behavior, edge_cases, side_effects, dependencies, error_modes, inputs, outputs, complexity, confidence, and model provenance."
    )]
    pub async fn aether_sir_inject(
        &self,
        Parameters(request): Parameters<AetherSirInjectRequest>,
    ) -> Result<Json<AetherSirInjectResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_sir_inject");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_sir_inject_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_sir_context",
        description = "Assemble token-budgeted context for a symbol including source, SIR, graph neighbors, health, reasoning trace, and test intents in one call"
    )]
    pub async fn aether_sir_context(
        &self,
        Parameters(request): Parameters<AetherSirContextRequest>,
    ) -> Result<Json<AetherSirContextResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_sir_context");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_sir_context_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_blast_radius",
        description = "Analyze coupled files and risk levels for blast-radius impact"
    )]
    pub async fn aether_blast_radius(
        &self,
        Parameters(request): Parameters<AetherBlastRadiusRequest>,
    ) -> Result<Json<AetherBlastRadiusResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_blast_radius");
        let graph = self.state.surreal_graph().await.ok();
        let server = self.clone();
        tokio::task::spawn_blocking(move || {
            server.aether_blast_radius_logic_with_graph(graph, request)
        })
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_test_intents",
        description = "Query extracted behavioral test intents for a file or symbol"
    )]
    pub async fn aether_test_intents(
        &self,
        Parameters(request): Parameters<AetherTestIntentsRequest>,
    ) -> Result<Json<AetherTestIntentsResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_test_intents");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_test_intents_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_drift_report",
        description = "Run semantic drift analysis with boundary and structural anomaly detection"
    )]
    pub async fn aether_drift_report(
        &self,
        Parameters(request): Parameters<AetherDriftReportRequest>,
    ) -> Result<Json<AetherDriftReportResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_drift_report");
        let graph = self.state.surreal_graph().await.ok();
        let server = self.clone();
        tokio::task::spawn_blocking(move || {
            server.aether_drift_report_logic_with_graph(graph, request)
        })
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_health",
        description = "Get codebase health metrics including critical symbols, bottlenecks, dependency cycles, orphaned code, and risk hotspots."
    )]
    pub async fn aether_health(
        &self,
        Parameters(request): Parameters<AetherHealthRequest>,
    ) -> Result<Json<AetherHealthResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_health");
        self.aether_health_logic(request)
            .await
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_health_hotspots",
        description = "Return the hottest workspace crates by health score with archetypes and top violations."
    )]
    pub async fn aether_health_hotspots(
        &self,
        Parameters(request): Parameters<AetherHealthHotspotsRequest>,
    ) -> Result<Json<AetherTextResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_health_hotspots");
        self.aether_health_hotspots_logic(request)
            .await
            .map(|text| Json(AetherTextResponse { text }))
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_health_explain",
        description = "Explain one crate's health score, signals, violations, and split suggestions."
    )]
    pub async fn aether_health_explain(
        &self,
        Parameters(request): Parameters<AetherHealthExplainRequest>,
    ) -> Result<Json<AetherTextResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_health_explain");
        self.aether_health_explain_logic(request)
            .await
            .map(|text| Json(AetherTextResponse { text }))
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_refactor_prep",
        description = "Prepare a file or crate for refactoring by deep-scanning the highest-risk symbols and saving an intent snapshot"
    )]
    pub async fn aether_refactor_prep(
        &self,
        Parameters(request): Parameters<AetherRefactorPrepRequest>,
    ) -> Result<Json<AetherRefactorPrepResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_refactor_prep");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_refactor_prep_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_verify_intent",
        description = "Compare current SIR against a saved refactor-prep snapshot and flag semantic drift"
    )]
    pub async fn aether_verify_intent(
        &self,
        Parameters(request): Parameters<AetherVerifyIntentRequest>,
    ) -> Result<Json<AetherVerifyIntentResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_verify_intent");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_verify_intent_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_trace_cause",
        description = "Trace likely upstream semantic causes of a downstream breakage"
    )]
    pub async fn aether_trace_cause(
        &self,
        Parameters(request): Parameters<AetherTraceCauseRequest>,
    ) -> Result<Json<AetherTraceCauseResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_trace_cause");
        let graph = self.state.surreal_graph().await.ok();
        let server = self.clone();
        tokio::task::spawn_blocking(move || {
            server.aether_trace_cause_logic_with_graph(graph, request)
        })
        .await
        .map_err(|err| McpError::internal_error(err.to_string(), None))?
        .map(Json)
        .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_acknowledge_drift",
        description = "Acknowledge drift findings and create a project note"
    )]
    pub async fn aether_acknowledge_drift(
        &self,
        Parameters(request): Parameters<AetherAcknowledgeDriftRequest>,
    ) -> Result<Json<AetherAcknowledgeDriftResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_acknowledge_drift");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_acknowledge_drift_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_symbol_timeline",
        description = "Get ordered SIR timeline entries for a symbol"
    )]
    pub async fn aether_symbol_timeline(
        &self,
        Parameters(request): Parameters<AetherSymbolTimelineRequest>,
    ) -> Result<Json<AetherSymbolTimelineResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_symbol_timeline");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_symbol_timeline_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_why_changed",
        description = "Explain why a symbol changed between two SIR versions or timestamps"
    )]
    pub async fn aether_why_changed(
        &self,
        Parameters(request): Parameters<AetherWhyChangedRequest>,
    ) -> Result<Json<AetherWhyChangedResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_why_changed");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_why_changed_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_get_sir",
        description = "Get SIR for leaf/file/module level"
    )]
    pub async fn aether_get_sir(
        &self,
        Parameters(request): Parameters<AetherGetSirRequest>,
    ) -> Result<Json<AetherGetSirResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_get_sir");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_get_sir_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }

    #[tool(
        name = "aether_explain",
        description = "Explain symbol at a file position using local SIR"
    )]
    pub async fn aether_explain(
        &self,
        Parameters(request): Parameters<AetherExplainRequest>,
    ) -> Result<Json<AetherExplainResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_explain");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_explain_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }
}

#[cfg(feature = "verification")]
impl AetherMcpServer {
    #[tool(
        name = "aether_verify",
        description = "Run allowlisted verification commands in host, container, or microvm mode"
    )]
    pub async fn aether_verify(
        &self,
        Parameters(request): Parameters<AetherVerifyRequest>,
    ) -> Result<Json<AetherVerifyResponse>, McpError> {
        self.verbose_log("MCP tool called: aether_verify");
        let server = self.clone();
        tokio::task::spawn_blocking(move || server.aether_verify_logic(request))
            .await
            .map_err(|err| McpError::internal_error(err.to_string(), None))?
            .map(Json)
            .map_err(to_mcp_error)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AetherMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: SERVER_NAME.to_owned(),
                title: None,
                version: SERVER_VERSION.to_owned(),
                icons: None,
                website_url: None,
            },
            instructions: Some(SERVER_DESCRIPTION.to_owned()),
            ..Default::default()
        }
    }
}
