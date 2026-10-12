#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchSpec {
    pub url: &'static str,
    pub dest_rel: &'static str,
    pub sha256: &'static str,
    pub size_hint: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub dest_rel: String,
    pub sha256: String,
    pub size_hint: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchProgress {
    pub received: u64,
    pub total: Option<u64>,
    pub speed_bps: u64,
}
