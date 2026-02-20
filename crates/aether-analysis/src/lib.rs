mod coupling;
mod drift;
mod test_intents;

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
pub use test_intents::{InferredTestTarget, TestGuard, TestIntentAnalyzer};
