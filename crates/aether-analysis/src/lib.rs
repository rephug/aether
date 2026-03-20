mod causal;
mod coupling;
mod drift;
mod graph_algorithms;
mod health;
mod refactor;
mod sir_quality_signals;
mod test_intents;

pub use causal::{
    CausalAnalyzer, CausalChainChange, CausalChainCoupling, CausalChainEntry, CausalChainSirDiff,
    TraceCauseAnalysisWindow, TraceCauseRequest, TraceCauseResult, TraceCauseTarget,
};
pub use coupling::{
    AnalysisError, BlastRadiusEntry, BlastRadiusRequest, BlastRadiusResult, CouplingAnalyzer,
    CouplingEdge, CouplingMiningOutcome, CouplingType, MineCouplingRequest, RiskLevel,
    SignalBreakdown,
};
pub use drift::{
    AcknowledgeDriftRequest, AcknowledgeDriftResult, BoundaryViolationEntry, CommunitiesRequest,
    CommunitiesResult, CommunityEntry, DriftAnalyzer, DriftInclude, DriftReportRequest,
    DriftReportResult, DriftReportSummary, DriftReportWindow, EmergingHubEntry, NewCycleEntry,
    OrphanedSubgraphEntry, SemanticDriftEntry, StructuralAnomalies,
};
pub use graph_algorithms::{
    GraphAlgorithmEdge, betweenness_centrality, bfs_shortest_path, connected_components,
    cross_community_edges, louvain_communities, page_rank, strongly_connected_components,
};
pub use health::*;
pub use refactor::*;
pub use sir_quality_signals::{
    SirQualitySignals, blend_normalized_quality, compute_confidence_percentiles,
    compute_sir_quality_signals,
};
pub use test_intents::{InferredTestTarget, TestGuard, TestIntentAnalyzer};
