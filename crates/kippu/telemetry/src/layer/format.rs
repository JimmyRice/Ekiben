/// How logs are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// A block per request, from arrival to response, for people.
    Pretty,
    /// One line per request.
    Compact,
    /// One JSON object per event, tagged with `request_id`, for log collectors.
    Json,
}
