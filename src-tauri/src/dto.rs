use serde::Serialize;

/// 传给前端的一行(camelCase 对齐 TS 类型)。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub line_no: u64,
    pub date: String,
    pub time: String,
    pub level: String,
    pub pid: String,
    pub tid: String,
    pub tag: String,
    pub message: String,
    pub marked: bool,
}

/// 会话状态快照。
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub total_lines: usize,
    pub indexed_bytes: u64,
    pub total_bytes: u64,
    pub indexing: bool,
}
