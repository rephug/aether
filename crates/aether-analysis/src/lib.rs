mod causal;
mod coupling;
mod drift;
mod health;
mod intent;
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
pub use health::{
    HealthAnalysisSummary, HealthAnalyzer, HealthBottleneckEntry, HealthCycleEntry,
    HealthCycleSymbol, HealthInclude, HealthOrphanEntry, HealthReportRequest, HealthReportResult,
    HealthRiskHotspotEntry, HealthSymbolEntry,
};
pub use intent::{
    IntentAnalyzer, IntentScope, IntentSnapshotRequest, IntentSnapshotResult, IntentStatus,
    IntentSymbolAddedEntry, IntentSymbolPreservedEntry, IntentSymbolRemovedEntry,
    IntentSymbolShiftedEntry, IntentTestCoverageGap, VerifyIntentRequest, VerifyIntentResult,
    VerifyIntentSummary,
};
pub use test_intents::{InferredTestTarget, TestGuard, TestIntentAnalyzer};
