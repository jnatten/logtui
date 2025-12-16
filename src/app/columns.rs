#[derive(Clone, Debug)]
pub struct ColumnDef {
    pub name: String,
    pub path: Vec<String>,
    pub enabled: bool,
}

impl ColumnDef {
    pub fn new(name: String, path: Vec<String>) -> Self {
        Self {
            name,
            path,
            enabled: false,
        }
    }
}

pub fn default_columns() -> Vec<ColumnDef> {
    vec![
        ColumnDef {
            name: "timestamp".into(),
            path: vec!["timestamp".into()],
            enabled: true,
        },
        ColumnDef {
            name: "level".into(),
            path: vec!["level".into()],
            enabled: true,
        },
        ColumnDef {
            name: "message".into(),
            path: vec!["message".into()],
            enabled: true,
        },
    ]
}

pub fn is_reserved_column(key: &str) -> bool {
    matches!(key, "timestamp" | "level" | "message" | "instant" | "data")
}
