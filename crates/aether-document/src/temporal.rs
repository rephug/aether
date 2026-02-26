pub trait ChangeEvent: Send + Sync {
    fn event_id(&self) -> &str;
    fn timestamp(&self) -> i64;
    fn source(&self) -> &str;
}
