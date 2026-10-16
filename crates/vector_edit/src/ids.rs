use uuid::Uuid;

pub fn new_id() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

pub fn is_pdf_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}
