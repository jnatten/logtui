use ratatui::widgets::ListState;
use serde_json::Value;

#[derive(Clone)]
pub struct FieldEntry {
    pub path: String,
    pub value: Value,
}

pub struct FieldViewState {
    pub fields: Vec<FieldEntry>,
    pub filtered_indices: Vec<usize>,
    pub list_state: ListState,
    pub filter: String,
}

impl FieldViewState {
    pub fn selected_field(&self) -> Option<&FieldEntry> {
        let idx = self.list_state.selected()?;
        let field_idx = *self.filtered_indices.get(idx)?;
        self.fields.get(field_idx)
    }

    pub fn rebuild_filter(&mut self) -> bool {
        let old_selection = self.list_state.selected();
        let filter = self.filter.to_lowercase();
        self.filtered_indices.clear();
        for (idx, field) in self.fields.iter().enumerate() {
            if filter.is_empty() || field.path.to_lowercase().contains(&filter) {
                self.filtered_indices.push(idx);
            }
        }
        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            match old_selection {
                Some(sel) if sel < self.filtered_indices.len() => {}
                _ => self.list_state.select(Some(0)),
            }
        }
        old_selection != self.list_state.selected()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldZoom {
    Detail,
}

pub fn collect_fields(value: &Value) -> Vec<FieldEntry> {
    fn walk(value: &Value, path: String, out: &mut Vec<FieldEntry>) {
        let display_path = if path.is_empty() {
            "(root)".to_string()
        } else {
            path.clone()
        };
        out.push(FieldEntry {
            path: display_path,
            value: value.clone(),
        });
        match value {
            Value::Object(map) => {
                for (key, v) in map {
                    let next = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };
                    walk(v, next, out);
                }
            }
            Value::Array(arr) => {
                for (idx, v) in arr.iter().enumerate() {
                    let next = if path.is_empty() {
                        format!("[{idx}]")
                    } else {
                        format!("{path}[{idx}]")
                    };
                    walk(v, next, out);
                }
            }
            _ => {}
        }
    }

    let mut out = Vec::new();
    walk(value, String::new(), &mut out);
    out
}

pub(crate) fn field_value_for_filter(entry: &FieldEntry) -> String {
    match &entry.value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}
